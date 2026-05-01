use super::*;

impl BrowserWorkerNode {
    pub(in crate::browser_worker) fn attach_data_channel(
        &self,
        session_key: &WorkerSessionKey,
        _source: WorkerDataChannelSource,
    ) -> BrowserWorkerResult<WorkerAttachDataChannelResult> {
        self.attach_transferred_channel(session_key)?;
        Ok(WorkerAttachDataChannelResult {
            session_key: session_key.as_str().to_owned(),
            attached: true,
            connection_key: None,
        })
    }

    pub(in crate::browser_worker) fn handle_bootstrap_signal(
        &self,
        input: WorkerBootstrapSignalInput,
    ) -> BrowserWorkerResult<WorkerBootstrapSignalResult> {
        match input.signal {
            WebRtcSignal::DialRequest {
                dial_id,
                from,
                to,
                generation,
                alpn,
                transport_intent,
            } => {
                self.ensure_signal_targets_local(to)?;
                let alpn = input
                    .alpn
                    .or_else(|| (!alpn.is_empty()).then_some(alpn))
                    .ok_or_else(|| {
                        BrowserWorkerError::new(
                            BrowserWorkerErrorCode::BootstrapFailed,
                            "bootstrap dial request is missing application ALPN",
                        )
                    })?;
                if !self.is_alpn_registered(&alpn) {
                    return Err(BrowserWorkerError::new(
                        BrowserWorkerErrorCode::UnsupportedAlpn,
                        format!("no active accept registration for ALPN {alpn:?}"),
                    ));
                }
                let session = self.allocate_accept_session(
                    from,
                    alpn,
                    DialSessionId {
                        dial_id,
                        generation,
                    },
                    transport_intent,
                )?;
                let session_key = session.session_key.as_str().to_owned();
                let main_rtc = if transport_intent.uses_webrtc() {
                    vec![self.create_peer_connection_command(
                        &session.session_key,
                        WorkerSessionRole::Acceptor,
                        generation,
                        from,
                    )]
                } else {
                    Vec::new()
                };
                Ok(WorkerBootstrapSignalResult {
                    accepted: true,
                    session_key: Some(session_key),
                    session: Some(session),
                    connection: None,
                    main_rtc,
                    outbound_signals: Vec::new(),
                })
            }
            signal if signal.is_terminal() => self.apply_remote_terminal_signal(signal),
            signal => {
                self.ensure_signal_targets_local(signal.to())?;
                let session_key = WorkerSessionKey::from(signal.dial_id());
                let session = self.session_snapshot(&session_key)?;
                if session.remote != signal.from() || session.generation != signal.generation() {
                    return Err(BrowserWorkerError::new(
                        BrowserWorkerErrorCode::BootstrapFailed,
                        "bootstrap signal does not match the worker session route",
                    ));
                }
                let main_rtc = self.main_rtc_commands_for_remote_signal(&session_key, &signal)?;
                Ok(WorkerBootstrapSignalResult {
                    accepted: true,
                    session_key: Some(session_key.as_str().to_owned()),
                    session: Some(session),
                    connection: None,
                    main_rtc,
                    outbound_signals: Vec::new(),
                })
            }
        }
    }

    fn main_rtc_commands_for_remote_signal(
        &self,
        session_key: &WorkerSessionKey,
        signal: &WebRtcSignal,
    ) -> BrowserWorkerResult<Vec<WorkerMainRtcCommand>> {
        let key = session_key.as_str().to_owned();
        match signal {
            WebRtcSignal::Offer { from, sdp, .. } => Ok(vec![WorkerMainRtcCommand::AcceptOffer(
                WorkerMainAcceptOfferPayload {
                    session_key: key.clone(),
                    remote_endpoint: Some(endpoint_id_string(*from)),
                    sdp: sdp.clone(),
                },
            )]),
            WebRtcSignal::Answer { sdp, .. } => Ok(vec![WorkerMainRtcCommand::AcceptAnswer(
                WorkerMainAcceptAnswerPayload {
                    session_key: key,
                    sdp: sdp.clone(),
                },
            )]),
            WebRtcSignal::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                ..
            } => Ok(vec![WorkerMainRtcCommand::AddRemoteIce(
                WorkerMainAddRemoteIcePayload {
                    session_key: key,
                    ice: WorkerProtocolIceCandidate::Candidate {
                        candidate: candidate.clone(),
                        sdp_mid: sdp_mid.clone(),
                        sdp_mline_index: *sdp_mline_index,
                    },
                },
            )]),
            WebRtcSignal::EndOfCandidates { .. } => Ok(vec![WorkerMainRtcCommand::AddRemoteIce(
                WorkerMainAddRemoteIcePayload {
                    session_key: key,
                    ice: WorkerProtocolIceCandidate::EndOfCandidates,
                },
            )]),
            _ => Ok(Vec::new()),
        }
    }

    pub(in crate::browser_worker) fn handle_main_rtc_result(
        &self,
        input: WorkerMainRtcResultInput,
    ) -> BrowserWorkerResult<WorkerMainRtcResult> {
        if input.error_message.is_some() {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::session",
                session_key = %input.session_key.as_str(),
                event = ?input.event,
                has_error = true,
                "handling main-thread RTC result"
            );
        } else {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::session",
                session_key = %input.session_key.as_str(),
                event = ?input.event,
                has_error = false,
                "handling main-thread RTC result"
            );
        }
        if let Some(message) = input.error_message {
            let decision = self.decide_webrtc_failure(&input.session_key, message)?;
            return Ok(WorkerMainRtcResult {
                accepted: true,
                session_key: input.session_key.as_str().to_owned(),
                session: Some(self.session_snapshot(&input.session_key)?),
                connection: None,
                main_rtc: close_peer_connection_commands(&decision),
                outbound_signals: decision.signal.into_iter().collect(),
            });
        }

        let session = self.session_snapshot(&input.session_key)?;
        let mut outbound_signals = Vec::new();
        let mut main_rtc = Vec::new();
        let connection = None;

        match input.event {
            WorkerMainRtcResultEvent::PeerConnectionCreated => {
                if session.role == WorkerSessionRole::Dialer {
                    main_rtc.push(WorkerMainRtcCommand::CreateDataChannel(
                        WorkerMainCreateDataChannelPayload {
                            session_key: input.session_key.as_str().to_owned(),
                            label: crate::core::signaling::WEBRTC_DATA_CHANNEL_LABEL.to_owned(),
                            protocol: crate::core::signaling::WEBRTC_DATA_CHANNEL_LABEL.to_owned(),
                            ordered: false,
                            max_retransmits: 0,
                        },
                    ));
                }
            }
            WorkerMainRtcResultEvent::DataChannelCreated => {
                if session.role == WorkerSessionRole::Dialer {
                    main_rtc.push(WorkerMainRtcCommand::CreateOffer(
                        WorkerMainSessionPayload {
                            session_key: input.session_key.as_str().to_owned(),
                        },
                    ));
                }
            }
            WorkerMainRtcResultEvent::OfferCreated { sdp } => {
                outbound_signals.push(WebRtcSignal::Offer {
                    dial_id: session.dial_id,
                    from: session.local,
                    to: session.remote,
                    generation: session.generation,
                    sdp,
                });
                main_rtc.push(WorkerMainRtcCommand::NextLocalIce(
                    WorkerMainSessionPayload {
                        session_key: input.session_key.as_str().to_owned(),
                    },
                ));
            }
            WorkerMainRtcResultEvent::OfferAccepted => {
                main_rtc.push(WorkerMainRtcCommand::TakeDataChannel(
                    WorkerMainSessionPayload {
                        session_key: input.session_key.as_str().to_owned(),
                    },
                ));
                main_rtc.push(WorkerMainRtcCommand::CreateAnswer(
                    WorkerMainSessionPayload {
                        session_key: input.session_key.as_str().to_owned(),
                    },
                ));
            }
            WorkerMainRtcResultEvent::AnswerCreated { sdp } => {
                outbound_signals.push(WebRtcSignal::Answer {
                    dial_id: session.dial_id,
                    from: session.local,
                    to: session.remote,
                    generation: session.generation,
                    sdp,
                });
                main_rtc.push(WorkerMainRtcCommand::NextLocalIce(
                    WorkerMainSessionPayload {
                        session_key: input.session_key.as_str().to_owned(),
                    },
                ));
            }
            WorkerMainRtcResultEvent::AnswerAccepted => {}
            WorkerMainRtcResultEvent::LocalIce { ice } => match ice {
                WorkerProtocolIceCandidate::Candidate {
                    candidate,
                    sdp_mid,
                    sdp_mline_index,
                } => {
                    outbound_signals.push(WebRtcSignal::IceCandidate {
                        dial_id: session.dial_id,
                        from: session.local,
                        to: session.remote,
                        generation: session.generation,
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                    });
                    main_rtc.push(WorkerMainRtcCommand::NextLocalIce(
                        WorkerMainSessionPayload {
                            session_key: input.session_key.as_str().to_owned(),
                        },
                    ));
                }
                WorkerProtocolIceCandidate::EndOfCandidates => {
                    outbound_signals.push(WebRtcSignal::EndOfCandidates {
                        dial_id: session.dial_id,
                        from: session.local,
                        to: session.remote,
                        generation: session.generation,
                    });
                }
            },
            WorkerMainRtcResultEvent::RemoteIceAccepted => {}
            WorkerMainRtcResultEvent::DataChannelOpen => {
                self.mark_transferred_channel_open(&input.session_key)?;
            }
            WorkerMainRtcResultEvent::WebRtcFailed { message } => {
                let decision = self.decide_webrtc_failure(&input.session_key, message)?;
                main_rtc.extend(close_peer_connection_commands(&decision));
                outbound_signals.extend(decision.signal);
            }
            WorkerMainRtcResultEvent::WebRtcClosed { message } => {
                let decision = self.close_session(&input.session_key, message)?;
                main_rtc.extend(close_peer_connection_commands(&decision));
                outbound_signals.extend(decision.signal);
            }
            WorkerMainRtcResultEvent::PeerConnectionCloseAcknowledged => {}
        }

        Ok(WorkerMainRtcResult {
            accepted: true,
            session_key: input.session_key.as_str().to_owned(),
            session: Some(self.session_snapshot(&input.session_key)?),
            connection,
            main_rtc,
            outbound_signals,
        })
    }

    pub(in crate::browser_worker) fn decide_webrtc_failure(
        &self,
        session_key: &WorkerSessionKey,
        message: impl Into<String>,
    ) -> BrowserWorkerResult<WorkerTerminalDecision> {
        self.emit_terminal_decision(session_key, None, message.into())
    }

    #[cfg(test)]
    pub(in crate::browser_worker) fn cancel_session(
        &self,
        session_key: &WorkerSessionKey,
        message: impl Into<String>,
    ) -> BrowserWorkerResult<WorkerTerminalDecision> {
        self.emit_terminal_decision(
            session_key,
            Some(WebRtcTerminalReason::Cancelled),
            message.into(),
        )
    }

    pub(in crate::browser_worker) fn close_session(
        &self,
        session_key: &WorkerSessionKey,
        message: impl Into<String>,
    ) -> BrowserWorkerResult<WorkerTerminalDecision> {
        self.emit_terminal_decision(
            session_key,
            Some(WebRtcTerminalReason::Closed),
            message.into(),
        )
    }

    pub(super) fn create_peer_connection_command(
        &self,
        session_key: &WorkerSessionKey,
        role: WorkerSessionRole,
        generation: u64,
        remote_endpoint: EndpointId,
    ) -> WorkerMainRtcCommand {
        WorkerMainRtcCommand::CreatePeerConnection(WorkerMainCreatePeerConnectionPayload {
            session_key: session_key.as_str().to_owned(),
            role,
            generation,
            stun_urls: self.session_config().ice.stun_urls,
            remote_endpoint: endpoint_id_string(remote_endpoint),
        })
    }

    fn ensure_signal_targets_local(&self, to: EndpointId) -> BrowserWorkerResult<()> {
        let local = self.endpoint_id();
        if to != local {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::BootstrapFailed,
                "bootstrap signal is not addressed to this worker node",
            ));
        }
        Ok(())
    }

    fn apply_remote_terminal_signal(
        &self,
        signal: WebRtcSignal,
    ) -> BrowserWorkerResult<WorkerBootstrapSignalResult> {
        self.ensure_signal_targets_local(signal.to())?;
        let reason = signal
            .terminal_reason()
            .expect("caller checks terminal signal");
        let session_key = WorkerSessionKey::from(signal.dial_id());
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        let session = inner.sessions.get_mut(&session_key).ok_or_else(|| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "unknown worker session key",
            )
        })?;
        if session.remote != signal.from() || session.generation != signal.generation() {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::BootstrapFailed,
                "terminal bootstrap signal does not match the worker session route",
            ));
        }

        session.record_terminal_received(reason);
        let snapshot = session.snapshot();
        Ok(WorkerBootstrapSignalResult {
            accepted: true,
            session_key: Some(session_key.as_str().to_owned()),
            session: Some(snapshot),
            connection: None,
            main_rtc: Vec::new(),
            outbound_signals: Vec::new(),
        })
    }

    fn emit_terminal_decision(
        &self,
        session_key: &WorkerSessionKey,
        reason_override: Option<WebRtcTerminalReason>,
        message: String,
    ) -> BrowserWorkerResult<WorkerTerminalDecision> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        let session = inner.sessions.get_mut(session_key).ok_or_else(|| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "unknown worker session key",
            )
        })?;
        let reason = reason_override.unwrap_or_else(|| {
            if session.transport_intent.allows_iroh_relay_fallback() {
                WebRtcTerminalReason::FallbackSelected
            } else {
                WebRtcTerminalReason::WebRtcFailed
            }
        });
        let signal = if session.transport_intent.uses_webrtc() {
            Some(session_terminal_signal(session, reason, Some(message)))
        } else {
            None
        };

        let selected_transport = session.record_terminal_sent(reason);

        Ok(WorkerTerminalDecision {
            session_key: session_key.as_str().to_owned(),
            reason,
            signal,
            selected_transport,
        })
    }
}
