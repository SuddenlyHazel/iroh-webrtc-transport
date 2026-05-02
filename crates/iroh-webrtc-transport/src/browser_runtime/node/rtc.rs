use super::*;

impl BrowserRuntimeNode {
    pub(in crate::browser_runtime) fn attach_data_channel(
        &self,
        session_key: &BrowserSessionKey,
        _source: DataChannelSource,
    ) -> BrowserRuntimeResult<AttachDataChannelOutcome> {
        self.attach_transferred_channel(session_key)?;
        Ok(AttachDataChannelOutcome {
            session_key: session_key.as_str().to_owned(),
            attached: true,
            connection_key: None,
        })
    }

    pub(in crate::browser_runtime) fn handle_bootstrap_signal(
        &self,
        input: BrowserBootstrapSignalInput,
    ) -> BrowserRuntimeResult<BrowserBootstrapSignalResult> {
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
                        BrowserRuntimeError::new(
                            BrowserRuntimeErrorCode::BootstrapFailed,
                            "bootstrap dial request is missing application ALPN",
                        )
                    })?;
                if !self.is_application_alpn_registered(&alpn) {
                    return Err(BrowserRuntimeError::new(
                        BrowserRuntimeErrorCode::UnsupportedAlpn,
                        format!("no registered application handler for ALPN {alpn:?}"),
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
                Ok(BrowserBootstrapSignalResult {
                    accepted: true,
                    session_key: Some(session_key),
                    session: Some(session),
                    connection: None,
                    outbound_signals: Vec::new(),
                })
            }
            signal if signal.is_terminal() => self.apply_remote_terminal_signal(signal),
            signal => {
                self.ensure_signal_targets_local(signal.to())?;
                let session_key = BrowserSessionKey::from(signal.dial_id());
                let session = self.session_snapshot(&session_key)?;
                if session.remote != signal.from() || session.generation != signal.generation() {
                    return Err(BrowserRuntimeError::new(
                        BrowserRuntimeErrorCode::BootstrapFailed,
                        "bootstrap signal does not match the browser session route",
                    ));
                }
                Ok(BrowserBootstrapSignalResult {
                    accepted: true,
                    session_key: Some(session_key.as_str().to_owned()),
                    session: Some(session),
                    connection: None,
                    outbound_signals: Vec::new(),
                })
            }
        }
    }

    pub(in crate::browser_runtime) fn handle_rtc_lifecycle_result(
        &self,
        input: RtcLifecycleInput,
    ) -> BrowserRuntimeResult<RtcLifecycleResult> {
        if input.error_message.is_some() {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::session",
                session_key = %input.session_key.as_str(),
                event = ?input.event,
                has_error = true,
                "handling main-thread RTC result"
            );
        } else {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_runtime::session",
                session_key = %input.session_key.as_str(),
                event = ?input.event,
                has_error = false,
                "handling main-thread RTC result"
            );
        }
        if let Some(message) = input.error_message {
            let decision = self.decide_webrtc_failure(&input.session_key, message)?;
            return Ok(RtcLifecycleResult {
                accepted: true,
                session_key: input.session_key.as_str().to_owned(),
                session: Some(self.session_snapshot(&input.session_key)?),
                connection: None,
                outbound_signals: decision.signal.into_iter().collect(),
            });
        }

        let mut outbound_signals = Vec::new();
        let connection = None;

        match input.event {
            RtcLifecycleEvent::DataChannelOpen => {
                self.mark_transferred_channel_open(&input.session_key)?;
            }
            RtcLifecycleEvent::WebRtcFailed { message } => {
                let decision = self.decide_webrtc_failure(&input.session_key, message)?;
                outbound_signals.extend(decision.signal);
            }
            #[cfg(all(target_arch = "wasm32", feature = "browser"))]
            RtcLifecycleEvent::WebRtcClosed { message } => {
                let decision = self.close_session(&input.session_key, message)?;
                outbound_signals.extend(decision.signal);
            }
        }

        Ok(RtcLifecycleResult {
            accepted: true,
            session_key: input.session_key.as_str().to_owned(),
            session: Some(self.session_snapshot(&input.session_key)?),
            connection,
            outbound_signals,
        })
    }

    pub(in crate::browser_runtime) fn decide_webrtc_failure(
        &self,
        session_key: &BrowserSessionKey,
        message: impl Into<String>,
    ) -> BrowserRuntimeResult<BrowserTerminalDecision> {
        self.emit_terminal_decision(session_key, None, message.into())
    }

    #[cfg(test)]
    pub(in crate::browser_runtime) fn cancel_session(
        &self,
        session_key: &BrowserSessionKey,
        message: impl Into<String>,
    ) -> BrowserRuntimeResult<BrowserTerminalDecision> {
        self.emit_terminal_decision(
            session_key,
            Some(WebRtcTerminalReason::Cancelled),
            message.into(),
        )
    }

    pub(in crate::browser_runtime) fn close_session(
        &self,
        session_key: &BrowserSessionKey,
        message: impl Into<String>,
    ) -> BrowserRuntimeResult<BrowserTerminalDecision> {
        self.emit_terminal_decision(
            session_key,
            Some(WebRtcTerminalReason::Closed),
            message.into(),
        )
    }

    fn ensure_signal_targets_local(&self, to: EndpointId) -> BrowserRuntimeResult<()> {
        let local = self.endpoint_id();
        if to != local {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::BootstrapFailed,
                "bootstrap signal is not addressed to this browser node",
            ));
        }
        Ok(())
    }

    fn apply_remote_terminal_signal(
        &self,
        signal: WebRtcSignal,
    ) -> BrowserRuntimeResult<BrowserBootstrapSignalResult> {
        self.ensure_signal_targets_local(signal.to())?;
        let reason = signal
            .terminal_reason()
            .expect("caller checks terminal signal");
        let session_key = BrowserSessionKey::from(signal.dial_id());
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        let session = inner.sessions.get_mut(&session_key).ok_or_else(|| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                "unknown browser session key",
            )
        })?;
        if session.remote != signal.from() || session.generation != signal.generation() {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::BootstrapFailed,
                "terminal bootstrap signal does not match the browser session route",
            ));
        }

        session.record_terminal_received(reason);
        let snapshot = session.snapshot();
        Ok(BrowserBootstrapSignalResult {
            accepted: true,
            session_key: Some(session_key.as_str().to_owned()),
            session: Some(snapshot),
            connection: None,
            outbound_signals: Vec::new(),
        })
    }

    fn emit_terminal_decision(
        &self,
        session_key: &BrowserSessionKey,
        reason_override: Option<WebRtcTerminalReason>,
        message: String,
    ) -> BrowserRuntimeResult<BrowserTerminalDecision> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        let session = inner.sessions.get_mut(session_key).ok_or_else(|| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                "unknown browser session key",
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

        Ok(BrowserTerminalDecision {
            session_key: session_key.as_str().to_owned(),
            reason,
            signal,
            selected_transport,
        })
    }
}
