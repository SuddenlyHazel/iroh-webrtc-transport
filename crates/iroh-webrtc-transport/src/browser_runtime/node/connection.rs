use super::*;

impl BrowserRuntimeNode {
    pub(in crate::browser_runtime) fn accept_open(
        &self,
        alpn: impl Into<String>,
    ) -> BrowserRuntimeResult<BrowserAcceptRegistration> {
        let alpn = validate_alpn(alpn.into())?;
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        let registration = inner.accepts.get_mut(&alpn).ok_or_else(|| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::UnsupportedAlpn,
                format!("ALPN {alpn:?} was not registered as a facade accept protocol"),
            )
        })?;
        if registration.claimed {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::UnsupportedAlpn,
                format!("an accept iterator is already active for ALPN {alpn:?}"),
            ));
        }
        registration.claimed = true;
        let registration = BrowserAcceptRegistration {
            id: registration.id,
            alpn: alpn.clone(),
        };
        Ok(registration)
    }

    pub(in crate::browser_runtime) fn accept_open_result(
        &self,
        alpn: impl Into<String>,
    ) -> BrowserRuntimeResult<BrowserAcceptRegistrationInfo> {
        let registration = self.accept_open(alpn)?;
        Ok(BrowserAcceptRegistrationInfo {
            accept_key: accept_key_string(registration.id),
            alpn: registration.alpn,
        })
    }

    pub(in crate::browser_runtime) async fn accept_next(
        &self,
        accept_id: BrowserAcceptId,
    ) -> BrowserRuntimeResult<BrowserAcceptNext> {
        let receiver = {
            let mut inner = self
                .inner
                .lock()
                .expect("browser runtime node mutex poisoned");
            if inner.closed {
                return Ok(BrowserAcceptNext::Done);
            }
            let registration = inner.accept_registration_mut(accept_id)?;
            if let Some(connection) = registration.queue.pop_front() {
                return Ok(BrowserAcceptNext::Ready(connection));
            }

            let (sender, receiver) = oneshot::channel();
            registration.waiters.push_back(sender);
            receiver
        };

        receiver.await.map_err(|_| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::Closed,
                "accept waiter was dropped before completion",
            )
        })
    }

    pub(in crate::browser_runtime) async fn accept_next_result(
        &self,
        accept_id: BrowserAcceptId,
    ) -> BrowserRuntimeResult<BrowserAcceptNextInfo> {
        match self.accept_next(accept_id).await? {
            BrowserAcceptNext::Ready(connection) => Ok(BrowserAcceptNextInfo {
                done: false,
                connection: Some(connection_info(&connection)),
            }),
            BrowserAcceptNext::Done => Ok(BrowserAcceptNextInfo {
                done: true,
                connection: None,
            }),
        }
    }

    #[cfg(test)]
    pub(in crate::browser_runtime) fn try_accept_next(
        &self,
        accept_id: BrowserAcceptId,
    ) -> BrowserRuntimeResult<Option<BrowserAcceptedConnection>> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Ok(None);
        }
        let registration = inner.accept_registration_mut(accept_id)?;
        Ok(registration.queue.pop_front())
    }

    pub(in crate::browser_runtime) fn accept_close(
        &self,
        accept_id: BrowserAcceptId,
    ) -> BrowserRuntimeResult<bool> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        let Some(alpn) = inner.accept_alpn_for_id(accept_id) else {
            return Ok(false);
        };
        if let Some(registration) = inner.accepts.get_mut(&alpn) {
            registration.close_acceptor();
        }
        Ok(true)
    }

    #[cfg(test)]
    pub(in crate::browser_runtime) fn is_alpn_registered(&self, alpn: &str) -> bool {
        self.inner
            .lock()
            .expect("browser runtime node mutex poisoned")
            .accepts
            .contains_key(alpn)
    }

    pub(in crate::browser_runtime) fn is_application_alpn_registered(&self, alpn: &str) -> bool {
        let inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        inner.accepts.contains_key(alpn)
            || inner
                .benchmark_echo_alpns
                .iter()
                .any(|registered| registered == alpn)
            || inner.browser_protocols.contains_alpn(alpn)
    }

    #[cfg(test)]
    pub(in crate::browser_runtime) fn admit_incoming_application_connection(
        &self,
        remote: EndpointId,
        alpn: impl Into<String>,
        transport: BrowserResolvedTransport,
        session_key: Option<BrowserSessionKey>,
    ) -> BrowserRuntimeResult<BrowserConnectionKey> {
        let alpn = validate_alpn(alpn.into())?;
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        if !inner.accepts.contains_key(&alpn) {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::UnsupportedAlpn,
                format!("no active accept registration for ALPN {alpn:?}"),
            ));
        }

        let key = BrowserConnectionKey(inner.next_connection_key);
        inner.next_connection_key = inner
            .next_connection_key
            .checked_add(1)
            .expect("browser connection id exhausted");
        let connection = BrowserAcceptedConnection {
            key,
            session_key: session_key.clone(),
            remote,
            alpn: alpn.clone(),
            transport,
        };
        let registration = inner
            .accepts
            .get_mut(&alpn)
            .expect("registration was checked above");
        registration.push_or_wake(connection.clone())?;
        inner.connections.insert(
            key,
            BrowserConnectionState {
                iroh_connection: None,
                session_key,
                transport,
                closed: false,
            },
        );
        Ok(key)
    }

    pub(in crate::browser_runtime) fn connection_close(
        &self,
        connection_key: BrowserConnectionKey,
        _reason: Option<String>,
    ) -> BrowserRuntimeResult<BrowserCloseOutcome> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        let Some(connection) = inner.connections.get_mut(&connection_key) else {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                "unknown browser connection key",
            ));
        };
        let session_key = connection.session_key.clone();
        let transport = connection.transport;
        connection.closed = true;
        if let Some(iroh_connection) = &connection.iroh_connection {
            iroh_connection.close(0u32.into(), b"browser connection closed");
        }
        if transport == BrowserResolvedTransport::WebRtc {
            if let Some(session_key) = session_key.as_ref() {
                if let Some(session) = inner.sessions.get_mut(session_key) {
                    session.close();
                }
            }
            inner.refresh_webrtc_transport_addrs();
        }
        Ok(BrowserCloseOutcome {
            closed: true,
            outbound_signals: Vec::new(),
        })
    }

    pub(super) fn bootstrap_connection_sender(&self) -> Option<mpsc::UnboundedSender<Connection>> {
        self.inner
            .lock()
            .expect("browser runtime node mutex poisoned")
            .bootstrap_connection_tx
            .clone()
    }

    pub(super) fn admit_iroh_application_connection(
        &self,
        iroh_connection: Connection,
        transport: BrowserResolvedTransport,
        session_key: Option<BrowserSessionKey>,
        queue_accept: bool,
    ) -> BrowserRuntimeResult<BrowserConnectionInfo> {
        self.admit_iroh_application_connection_with_key(
            iroh_connection,
            transport,
            session_key,
            queue_accept,
        )
        .map(|(_, info)| info)
    }

    pub(super) fn admit_iroh_application_connection_with_key(
        &self,
        iroh_connection: Connection,
        transport: BrowserResolvedTransport,
        session_key: Option<BrowserSessionKey>,
        queue_accept: bool,
    ) -> BrowserRuntimeResult<(BrowserConnectionKey, BrowserConnectionInfo)> {
        let remote = iroh_connection.remote_id();
        let alpn = String::from_utf8_lossy(iroh_connection.alpn()).to_string();
        trace_iroh_connection_paths(
            "admitting Iroh application connection",
            None,
            &iroh_connection,
        );
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        if queue_accept && !inner.accepts.contains_key(&alpn) {
            iroh_connection.close(0u32.into(), b"unsupported ALPN");
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::UnsupportedAlpn,
                format!("no active accept registration for ALPN {alpn:?}"),
            ));
        }
        let stored_session_key = session_key.clone();
        if let Some(session_key) = stored_session_key.as_ref() {
            if let Some(session) = inner.sessions.get(session_key) {
                validate_transport_resolution(session, transport)?;
            }
        }
        if transport == BrowserResolvedTransport::WebRtc {
            let session_key = stored_session_key.as_ref().ok_or_else(|| {
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    "WebRTC connection admission requires a session key",
                )
            })?;
            let session = inner.sessions.get(session_key).ok_or_else(|| {
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    "WebRTC connection admission has no matching session",
                )
            })?;
            require_webrtc_selected_path(&iroh_connection, session.remote, session.dial_id)?;
        }

        let key = inner.allocate_connection_key();
        let connection = BrowserAcceptedConnection {
            key,
            session_key,
            remote,
            alpn: alpn.clone(),
            transport,
        };
        if queue_accept {
            let registration = inner
                .accepts
                .get_mut(&alpn)
                .expect("registration was checked above");
            registration.push_or_wake(connection.clone())?;
        }
        let info = connection_info(&connection);
        inner.connections.insert(
            key,
            BrowserConnectionState {
                iroh_connection: Some(iroh_connection),
                session_key: stored_session_key.clone(),
                transport,
                closed: false,
            },
        );
        if let Some(session_key) = stored_session_key {
            if let Some(session) = inner.sessions.get_mut(&session_key) {
                session.resolve_transport(transport)?;
            }
        }
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::connection",
            connection_key = key.0,
            remote = %remote,
            alpn = %alpn,
            transport = ?transport,
            queue_accept,
            "admitted Iroh application connection"
        );
        if let Some(connection) = inner
            .connections
            .get(&key)
            .and_then(|connection| connection.iroh_connection.as_ref())
        {
            trace_iroh_connection_paths(
                "admitted Iroh application connection",
                Some(key.0),
                connection,
            );
        }
        Ok((key, info))
    }

    pub(super) fn iroh_connection(
        &self,
        connection_key: BrowserConnectionKey,
    ) -> BrowserRuntimeResult<Connection> {
        let inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        let connection = inner.connections.get(&connection_key).ok_or_else(|| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                "unknown browser connection key",
            )
        })?;
        if connection.closed {
            return Err(BrowserRuntimeError::closed());
        }
        connection.iroh_connection.clone().ok_or_else(|| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                "browser connection is not backed by an Iroh application connection",
            )
        })
    }
}
