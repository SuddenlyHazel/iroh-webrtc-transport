use super::*;

impl BrowserRuntimeNode {
    pub(in crate::browser_runtime) async fn dial_application_connection(
        &self,
        remote_addr: EndpointAddr,
        alpn: impl Into<String>,
        transport_intent: BootstrapTransportIntent,
    ) -> BrowserRuntimeResult<BrowserConnectionInfo> {
        let alpn = validate_alpn(alpn.into())?;
        {
            let inner = self
                .inner
                .lock()
                .expect("browser runtime node mutex poisoned");
            if inner.closed {
                return Err(BrowserRuntimeError::closed());
            }
        }

        match transport_intent {
            BootstrapTransportIntent::IrohRelay => {
                let allocation =
                    self.allocate_dial(remote_addr.id, alpn.clone(), transport_intent)?;
                self.dial_iroh_application_connection(
                    remote_addr,
                    alpn,
                    Some(allocation.session_key),
                )
                .await
            }
            BootstrapTransportIntent::WebRtcPreferred => {
                let start =
                    self.allocate_dial_start(remote_addr.id, alpn.clone(), transport_intent)?;
                let decision = self.decide_webrtc_failure(
                    &BrowserSessionKey::new(start.session_key.clone())?,
                    "main-thread RTC control path did not produce an application DataChannel before fallback",
                )?;
                if decision.selected_transport == Some(BrowserResolvedTransport::IrohRelay) {
                    self.dial_iroh_application_connection(
                        remote_addr,
                        alpn,
                        Some(BrowserSessionKey::new(start.session_key)?),
                    )
                    .await
                } else {
                    Err(BrowserRuntimeError::new(
                        BrowserRuntimeErrorCode::WebRtcFailed,
                        "WebRTC dial failed and relay fallback is not allowed",
                    ))
                }
            }
            BootstrapTransportIntent::WebRtcOnly => {
                let start = self.allocate_dial_start(remote_addr.id, alpn, transport_intent)?;
                let _ = self.decide_webrtc_failure(
                    &BrowserSessionKey::new(start.session_key)?,
                    "main-thread RTC control path did not produce an application DataChannel",
                )?;
                Err(BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    "WebRTC-only dial failed; relay application data is disabled",
                ))
            }
        }
    }

    pub(in crate::browser_runtime) fn allocate_dial(
        &self,
        remote: EndpointId,
        alpn: impl Into<String>,
        transport_intent: BootstrapTransportIntent,
    ) -> BrowserRuntimeResult<BrowserDialAllocation> {
        let alpn = validate_alpn(alpn.into())?;
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }

        let DialSessionId {
            dial_id,
            generation,
        } = inner.dial_ids.next(remote);
        let session_key = BrowserSessionKey::from(dial_id);
        let allocation = BrowserDialAllocation {
            session_key: session_key.clone(),
            dial_id,
            generation,
            local: inner.endpoint_id,
            remote,
            alpn: alpn.clone(),
            transport_intent,
        };
        let local = inner.endpoint_id;

        let state = BrowserSessionState::new(
            session_key.clone(),
            dial_id,
            generation,
            local,
            remote,
            alpn,
            BrowserSessionRole::Dialer,
            transport_intent,
        );
        let snapshot = state.snapshot();
        inner.sessions.insert(session_key, state);
        if transport_intent.uses_webrtc() {
            inner.refresh_webrtc_transport_addrs();
        }
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::session",
            session_key = %snapshot.session_key.as_str(),
            dial_id = ?snapshot.dial_id,
            generation = snapshot.generation,
            local = %snapshot.local,
            remote = %snapshot.remote,
            alpn = %snapshot.alpn,
            transport_intent = ?snapshot.transport_intent,
            channel_attachment = ?snapshot.channel_attachment,
            lifecycle = ?snapshot.lifecycle,
            "allocated dialer WebRTC session"
        );
        if transport_intent.uses_webrtc() {
            let local_session_addr = WebRtcAddr::session(local, dial_id.0).to_custom_addr();
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_runtime::session",
                session_key = %snapshot.session_key.as_str(),
                local_session_addr = ?local_session_addr,
                "advertised dialer WebRTC session local address"
            );
        }

        Ok(allocation)
    }

    pub(in crate::browser_runtime) fn allocate_dial_start(
        &self,
        remote: EndpointId,
        alpn: impl Into<String>,
        transport_intent: BootstrapTransportIntent,
    ) -> BrowserRuntimeResult<BrowserDialStart> {
        let allocation = self.allocate_dial(remote, alpn, transport_intent)?;
        let bootstrap_signal = WebRtcSignal::dial_request_with_alpn(
            allocation.dial_id,
            allocation.local,
            allocation.remote,
            allocation.generation,
            allocation.alpn.clone(),
            allocation.transport_intent,
        );
        Ok(BrowserDialStart {
            session_key: allocation.session_key.as_str().to_owned(),
            dial_id: dial_id_string(allocation.dial_id),
            generation: allocation.generation,
            remote_endpoint: endpoint_id_string(allocation.remote),
            alpn: allocation.alpn,
            transport_intent: allocation.transport_intent,
            bootstrap_signal,
        })
    }

    pub(in crate::browser_runtime) fn allocate_accept_session(
        &self,
        remote: EndpointId,
        alpn: impl Into<String>,
        dial_session: DialSessionId,
        transport_intent: BootstrapTransportIntent,
    ) -> BrowserRuntimeResult<BrowserSessionSnapshot> {
        let alpn = validate_alpn(alpn.into())?;
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }

        let session_key = BrowserSessionKey::from(dial_session.dial_id);
        if inner.sessions.contains_key(&session_key) {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                "browser session key is already allocated",
            ));
        }
        let state = BrowserSessionState::new(
            session_key.clone(),
            dial_session.dial_id,
            dial_session.generation,
            inner.endpoint_id,
            remote,
            alpn,
            BrowserSessionRole::Acceptor,
            transport_intent,
        );
        let snapshot = state.snapshot();
        inner.sessions.insert(session_key, state);
        if transport_intent.uses_webrtc() {
            inner.refresh_webrtc_transport_addrs();
        }
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::session",
            session_key = %snapshot.session_key.as_str(),
            dial_id = ?snapshot.dial_id,
            generation = snapshot.generation,
            local = %snapshot.local,
            remote = %snapshot.remote,
            alpn = %snapshot.alpn,
            transport_intent = ?snapshot.transport_intent,
            channel_attachment = ?snapshot.channel_attachment,
            lifecycle = ?snapshot.lifecycle,
            "allocated acceptor WebRTC session"
        );
        if transport_intent.uses_webrtc() {
            let local_session_addr =
                WebRtcAddr::session(snapshot.local, snapshot.dial_id.0).to_custom_addr();
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_runtime::session",
                session_key = %snapshot.session_key.as_str(),
                local_session_addr = ?local_session_addr,
                "advertised acceptor WebRTC session local address"
            );
        }
        Ok(snapshot)
    }

    pub(in crate::browser_runtime) fn attach_transferred_channel(
        &self,
        session_key: &BrowserSessionKey,
    ) -> BrowserRuntimeResult<BrowserSessionSnapshot> {
        let snapshot =
            self.update_session(session_key, BrowserSessionState::attach_transferred_channel)?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::session",
            session_key = %snapshot.session_key.as_str(),
            role = ?snapshot.role,
            remote = %snapshot.remote,
            channel_attachment = ?snapshot.channel_attachment,
            lifecycle = ?snapshot.lifecycle,
            "attached transferred RTCDataChannel to session"
        );
        Ok(snapshot)
    }

    pub(in crate::browser_runtime) fn mark_transferred_channel_open(
        &self,
        session_key: &BrowserSessionKey,
    ) -> BrowserRuntimeResult<BrowserSessionSnapshot> {
        let snapshot = self.update_session(
            session_key,
            BrowserSessionState::mark_transferred_channel_open,
        )?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::session",
            session_key = %snapshot.session_key.as_str(),
            role = ?snapshot.role,
            remote = %snapshot.remote,
            channel_attachment = ?snapshot.channel_attachment,
            lifecycle = ?snapshot.lifecycle,
            "marked transferred RTCDataChannel open"
        );
        Ok(snapshot)
    }

    pub(in crate::browser_runtime) fn complete_dial(
        &self,
        session_key: &BrowserSessionKey,
        resolved_transport: BrowserResolvedTransport,
    ) -> BrowserRuntimeResult<BrowserSessionSnapshot> {
        let snapshot = self.update_session(session_key, |session| {
            session.complete_dial(resolved_transport)
        })?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::session",
            session_key = %snapshot.session_key.as_str(),
            role = ?snapshot.role,
            remote = %snapshot.remote,
            resolved_transport = ?snapshot.resolved_transport,
            channel_attachment = ?snapshot.channel_attachment,
            lifecycle = ?snapshot.lifecycle,
            "completed WebRTC session dial"
        );
        Ok(snapshot)
    }

    pub(in crate::browser_runtime) fn session_snapshot(
        &self,
        session_key: &BrowserSessionKey,
    ) -> BrowserRuntimeResult<BrowserSessionSnapshot> {
        let inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        inner
            .sessions
            .get(session_key)
            .map(BrowserSessionState::snapshot)
            .ok_or_else(|| {
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    "unknown browser session key",
                )
            })
    }
}
