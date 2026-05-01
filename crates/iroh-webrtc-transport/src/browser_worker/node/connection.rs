use super::*;

impl BrowserWorkerNode {
    pub(in crate::browser_worker) fn accept_open(
        &self,
        alpn: impl Into<String>,
    ) -> BrowserWorkerResult<WorkerAcceptRegistration> {
        let alpn = validate_alpn(alpn.into())?;
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        if inner.accepts.contains_key(&alpn) {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::UnsupportedAlpn,
                format!("an accept iterator is already active for ALPN {alpn:?}"),
            ));
        }

        let id = WorkerAcceptId(inner.next_accept_id);
        inner.next_accept_id = inner
            .next_accept_id
            .checked_add(1)
            .expect("worker accept id exhausted");
        let registration = WorkerAcceptRegistration {
            id,
            alpn: alpn.clone(),
        };
        let capacity = inner.accept_queue_capacity;
        inner.accepts.insert(
            alpn.clone(),
            AcceptRegistrationState::new(id, alpn, capacity),
        );
        inner.refresh_endpoint_alpns();
        Ok(registration)
    }

    pub(in crate::browser_worker) fn accept_open_result(
        &self,
        alpn: impl Into<String>,
    ) -> BrowserWorkerResult<WorkerAcceptOpenResult> {
        let registration = self.accept_open(alpn)?;
        Ok(WorkerAcceptOpenResult {
            accept_key: accept_key_string(registration.id),
            alpn: registration.alpn,
        })
    }

    pub(in crate::browser_worker) async fn accept_next(
        &self,
        accept_id: WorkerAcceptId,
    ) -> BrowserWorkerResult<WorkerAcceptNext> {
        let receiver = {
            let mut inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            if inner.closed {
                return Ok(WorkerAcceptNext::Done);
            }
            let registration = inner.accept_registration_mut(accept_id)?;
            if let Some(connection) = registration.queue.pop_front() {
                return Ok(WorkerAcceptNext::Ready(connection));
            }

            let (sender, receiver) = oneshot::channel();
            registration.waiters.push_back(sender);
            receiver
        };

        receiver.await.map_err(|_| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::Closed,
                "accept waiter was dropped before completion",
            )
        })
    }

    pub(in crate::browser_worker) async fn accept_next_result(
        &self,
        accept_id: WorkerAcceptId,
    ) -> BrowserWorkerResult<WorkerAcceptNextResult> {
        match self.accept_next(accept_id).await? {
            WorkerAcceptNext::Ready(connection) => Ok(WorkerAcceptNextResult {
                done: false,
                connection: Some(connection_info(&connection)),
            }),
            WorkerAcceptNext::Done => Ok(WorkerAcceptNextResult {
                done: true,
                connection: None,
            }),
        }
    }

    #[cfg(test)]
    pub(in crate::browser_worker) fn try_accept_next(
        &self,
        accept_id: WorkerAcceptId,
    ) -> BrowserWorkerResult<Option<WorkerAcceptedConnection>> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Ok(None);
        }
        let registration = inner.accept_registration_mut(accept_id)?;
        Ok(registration.queue.pop_front())
    }

    pub(in crate::browser_worker) fn accept_close(
        &self,
        accept_id: WorkerAcceptId,
    ) -> BrowserWorkerResult<bool> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        let Some(alpn) = inner.accept_alpn_for_id(accept_id) else {
            return Ok(false);
        };
        if let Some(registration) = inner.accepts.remove(&alpn) {
            registration.complete_waiters_done();
        }
        inner.refresh_endpoint_alpns();
        Ok(true)
    }

    pub(in crate::browser_worker) fn is_alpn_registered(&self, alpn: &str) -> bool {
        self.inner
            .lock()
            .expect("browser worker node mutex poisoned")
            .accepts
            .contains_key(alpn)
    }

    #[cfg(test)]
    pub(in crate::browser_worker) fn admit_incoming_application_connection(
        &self,
        remote: EndpointId,
        alpn: impl Into<String>,
        transport: WorkerResolvedTransport,
        session_key: Option<WorkerSessionKey>,
    ) -> BrowserWorkerResult<WorkerConnectionKey> {
        let alpn = validate_alpn(alpn.into())?;
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        if !inner.accepts.contains_key(&alpn) {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::UnsupportedAlpn,
                format!("no active accept registration for ALPN {alpn:?}"),
            ));
        }

        let key = WorkerConnectionKey(inner.next_connection_key);
        inner.next_connection_key = inner
            .next_connection_key
            .checked_add(1)
            .expect("worker connection id exhausted");
        let connection = WorkerAcceptedConnection {
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
            WorkerConnectionState {
                iroh_connection: None,
                session_key,
                transport,
                closed: false,
            },
        );
        Ok(key)
    }

    pub(in crate::browser_worker) fn connection_close(
        &self,
        connection_key: WorkerConnectionKey,
        _reason: Option<String>,
    ) -> BrowserWorkerResult<WorkerCloseResult> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        let Some(connection) = inner.connections.get_mut(&connection_key) else {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "unknown worker connection key",
            ));
        };
        let session_key = connection.session_key.clone();
        let transport = connection.transport;
        connection.closed = true;
        if let Some(iroh_connection) = &connection.iroh_connection {
            iroh_connection.close(0u32.into(), b"worker connection closed");
        }
        if transport == WorkerResolvedTransport::WebRtc {
            if let Some(session_key) = session_key.as_ref() {
                if let Some(session) = inner.sessions.get_mut(session_key) {
                    session.close();
                }
            }
            inner.refresh_webrtc_transport_addrs();
        }
        Ok(WorkerCloseResult {
            closed: true,
            outbound_signals: Vec::new(),
        })
    }

    pub(super) fn bootstrap_connection_sender(&self) -> Option<mpsc::UnboundedSender<Connection>> {
        self.inner
            .lock()
            .expect("browser worker node mutex poisoned")
            .bootstrap_connection_tx
            .clone()
    }

    pub(super) fn incoming_application_route(
        &self,
        remote: EndpointId,
        alpn: &str,
    ) -> (WorkerResolvedTransport, Option<WorkerSessionKey>) {
        let inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        let session_key = inner
            .sessions
            .values()
            .find(|session| {
                session.role == WorkerSessionRole::Acceptor
                    && session.remote == remote
                    && session.alpn == alpn
                    && session.transport_intent.uses_webrtc()
                    && session.resolved_transport.is_none()
                    && session.channel_attachment == WorkerDataChannelAttachmentState::Open
                    && matches!(
                        session.lifecycle,
                        WorkerSessionLifecycle::WebRtcNegotiating
                            | WorkerSessionLifecycle::ApplicationReady
                    )
            })
            .map(|session| session.session_key.clone());
        match session_key {
            Some(session_key) => {
                tracing::trace!(
                    target: "iroh_webrtc_transport::browser_worker::connection",
                    remote = %remote,
                    alpn = %alpn,
                    session_key = %session_key.as_str(),
                    "matched incoming application connection to WebRTC session"
                );
                (WorkerResolvedTransport::WebRtc, Some(session_key))
            }
            None => {
                tracing::trace!(
                    target: "iroh_webrtc_transport::browser_worker::connection",
                    remote = %remote,
                    alpn = %alpn,
                    "routing incoming application connection as Iroh relay"
                );
                (WorkerResolvedTransport::IrohRelay, None)
            }
        }
    }

    pub(super) fn admit_iroh_application_connection(
        &self,
        iroh_connection: Connection,
        transport: WorkerResolvedTransport,
        session_key: Option<WorkerSessionKey>,
        queue_accept: bool,
    ) -> BrowserWorkerResult<WorkerProtocolConnectionInfo> {
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
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        if queue_accept && !inner.accepts.contains_key(&alpn) {
            iroh_connection.close(0u32.into(), b"unsupported ALPN");
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::UnsupportedAlpn,
                format!("no active accept registration for ALPN {alpn:?}"),
            ));
        }
        let stored_session_key = session_key.clone();
        if let Some(session_key) = stored_session_key.as_ref() {
            if let Some(session) = inner.sessions.get(session_key) {
                validate_transport_resolution(session, transport)?;
            }
        }
        if transport == WorkerResolvedTransport::WebRtc {
            let session_key = stored_session_key.as_ref().ok_or_else(|| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    "WebRTC connection admission requires a session key",
                )
            })?;
            let session = inner.sessions.get(session_key).ok_or_else(|| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    "WebRTC connection admission has no matching session",
                )
            })?;
            require_webrtc_selected_path(&iroh_connection, session.remote, session.dial_id)?;
        }

        let key = inner.allocate_connection_key();
        let connection = WorkerAcceptedConnection {
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
            WorkerConnectionState {
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
            target: "iroh_webrtc_transport::browser_worker::connection",
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
        Ok(info)
    }

    pub(super) fn iroh_connection(
        &self,
        connection_key: WorkerConnectionKey,
    ) -> BrowserWorkerResult<Connection> {
        let inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        let connection = inner.connections.get(&connection_key).ok_or_else(|| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "unknown worker connection key",
            )
        })?;
        if connection.closed {
            return Err(BrowserWorkerError::closed());
        }
        connection.iroh_connection.clone().ok_or_else(|| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "worker connection is not backed by an Iroh application connection",
            )
        })
    }
}
