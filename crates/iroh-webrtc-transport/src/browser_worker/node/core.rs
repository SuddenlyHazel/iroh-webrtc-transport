use super::*;
use iroh::endpoint::{AckFrequencyConfig, QuicTransportConfig, VarInt};
use std::time::Duration;

impl BrowserWorkerNode {
    #[cfg(test)]
    pub(in crate::browser_worker) fn spawn(
        config: BrowserWorkerNodeConfig,
    ) -> BrowserWorkerResult<Self> {
        Self::spawn_with_secret_key(
            config,
            SecretKey::generate(),
            BrowserWorkerProtocolRegistry::default(),
        )
    }

    #[cfg(test)]
    pub(in crate::browser_worker) fn spawn_default() -> BrowserWorkerResult<Self> {
        Self::spawn(BrowserWorkerNodeConfig::default())
    }

    pub(in crate::browser_worker) fn spawn_with_secret_key(
        config: BrowserWorkerNodeConfig,
        secret_key: SecretKey,
        worker_protocols: BrowserWorkerProtocolRegistry,
    ) -> BrowserWorkerResult<Self> {
        config.validate()?;
        let facade_alpns = config
            .facade_alpns
            .iter()
            .map(|alpn| validate_alpn(alpn.clone()))
            .collect::<BrowserWorkerResult<Vec<_>>>()?;
        let benchmark_echo_alpns = config
            .benchmark_echo_alpns
            .iter()
            .map(|alpn| validate_alpn(alpn.clone()))
            .collect::<BrowserWorkerResult<Vec<_>>>()?;
        let mut seen_alpns = std::collections::HashSet::new();
        for alpn in &facade_alpns {
            if !seen_alpns.insert(alpn.clone())
                || worker_protocols.contains_alpn(alpn)
                || benchmark_echo_alpns.contains(alpn)
            {
                return Err(BrowserWorkerError::new(
                    BrowserWorkerErrorCode::UnsupportedAlpn,
                    format!("duplicate worker ALPN registration {alpn:?}"),
                ));
            }
        }
        for alpn in &benchmark_echo_alpns {
            if !seen_alpns.insert(alpn.clone()) || worker_protocols.contains_alpn(alpn) {
                return Err(BrowserWorkerError::new(
                    BrowserWorkerErrorCode::UnsupportedAlpn,
                    format!("duplicate worker ALPN registration {alpn:?}"),
                ));
            }
        }
        let endpoint_id = secret_key.public();
        let local_custom_addr = config
            .transport_config
            .local_addr
            .clone()
            .unwrap_or_else(|| WebRtcAddr::capability(endpoint_id).to_custom_addr());
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::node",
            endpoint_id = %endpoint_id,
            local_custom_addr = ?local_custom_addr,
            accept_queue_capacity = config.accept_queue_capacity,
            "creating browser worker node"
        );
        let mut transport_config = config.transport_config;
        transport_config.local_addr = Some(local_custom_addr.clone());
        let transport = WebRtcTransport::try_new(transport_config).map_err(|err| {
            BrowserWorkerError::new(BrowserWorkerErrorCode::SpawnFailed, err.to_string())
        })?;

        let accepts = initial_accept_registrations(facade_alpns, config.accept_queue_capacity);
        let protocol_transport_lookup =
            MemoryLookup::with_provenance("iroh_webrtc_transport_protocol_sessions");

        Ok(Self {
            inner: Arc::new(Mutex::new(BrowserWorkerNodeInner {
                secret_key,
                node_key: WorkerNodeKey(format!("node-{}", endpoint_id)),
                endpoint_id,
                local_custom_addr,
                transport,
                relay_endpoint: None,
                relay_router: None,
                webrtc_endpoint: None,
                webrtc_router: None,
                benchmark_echo_tasks: HashMap::new(),
                bootstrap_connection_tx: None,
                worker_protocols,
                benchmark_echo_alpns,
                worker_protocol_transport_intent: config.worker_protocol_transport_intent,
                protocol_transport_lookup,
                protocol_transport_prepare_tx: config.protocol_transport_prepare_tx,
                dial_ids: DialIdGenerator::new(endpoint_id),
                session_config: config.session_config,
                low_latency_quic_acks: config.low_latency_quic_acks,
                sessions: HashMap::new(),
                connections: HashMap::new(),
                streams: HashMap::new(),
                accepts,
                next_connection_key: 1,
                next_stream_key: 1,
                closed: false,
            })),
        })
    }

    #[cfg(test)]
    pub(in crate::browser_worker) fn state(&self) -> BrowserWorkerNodeState {
        let inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        BrowserWorkerNodeState {
            endpoint_id: inner.endpoint_id,
            local_custom_addr: inner.local_custom_addr.clone(),
            bootstrap_alpn: bootstrap_alpn_str(),
            closed: inner.closed,
        }
    }

    pub(in crate::browser_worker) fn endpoint_id(&self) -> EndpointId {
        self.inner
            .lock()
            .expect("browser worker node mutex poisoned")
            .endpoint_id
    }

    pub(in crate::browser_worker) fn session_config(&self) -> WebRtcSessionConfig {
        self.inner
            .lock()
            .expect("browser worker node mutex poisoned")
            .session_config
            .clone()
    }

    pub(in crate::browser_worker) fn session_hub(&self) -> SessionHub {
        self.inner
            .lock()
            .expect("browser worker node mutex poisoned")
            .transport
            .session_hub()
    }

    pub(in crate::browser_worker) fn spawn_result(&self) -> WorkerSpawnResult {
        let inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        WorkerSpawnResult {
            node_key: inner.node_key.as_str().to_owned(),
            endpoint_id: endpoint_id_string(inner.endpoint_id),
            local_custom_addr: custom_addr_string(&inner.local_custom_addr),
            bootstrap_alpn: bootstrap_alpn_str().to_owned(),
        }
    }

    pub(in crate::browser_worker) fn is_closed(&self) -> bool {
        self.inner
            .lock()
            .expect("browser worker node mutex poisoned")
            .closed
    }

    pub(in crate::browser_worker) fn set_bootstrap_connection_sender(
        &self,
        sender: Option<mpsc::UnboundedSender<Connection>>,
    ) -> BrowserWorkerResult<()> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        inner.bootstrap_connection_tx = sender;
        Ok(())
    }

    pub(in crate::browser_worker) async fn start_endpoint(&self) -> BrowserWorkerResult<()> {
        let (secret_key, transport, low_latency_quic_acks) = {
            let inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            if inner.closed {
                return Err(BrowserWorkerError::closed());
            }
            if inner.relay_endpoint.is_some() && inner.webrtc_endpoint.is_some() {
                return Ok(());
            }
            (
                inner.secret_key.clone(),
                inner.transport.clone(),
                inner.low_latency_quic_acks,
            )
        };

        let (protocol_transport_lookup, protocol_transport_hook) = {
            let inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            (
                inner.protocol_transport_lookup.clone(),
                self.protocol_transport_hook(),
            )
        };
        let mut relay_builder =
            Endpoint::builder(iroh::endpoint::presets::N0).secret_key(secret_key.clone());
        if low_latency_quic_acks {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::node",
                "enabling low-latency QUIC ACK frequency config"
            );
            let low_latency_config = low_latency_ack_transport_config();
            relay_builder = relay_builder.transport_config(low_latency_config.clone());
        }
        let relay_endpoint = relay_builder.bind().await.map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::SpawnFailed,
                format!("failed to bind worker relay Iroh endpoint: {err}"),
            )
        })?;
        let webrtc_endpoint = bind_webrtc_endpoint(
            secret_key,
            transport,
            low_latency_quic_acks,
            Some(protocol_transport_lookup),
            Some(protocol_transport_hook),
        )
        .await?;
        let (relay_router, webrtc_router) =
            self.build_endpoint_routers(relay_endpoint.clone(), webrtc_endpoint.clone())?;

        let mut close_endpoints = false;
        {
            let mut inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            if inner.closed || inner.relay_endpoint.is_some() || inner.webrtc_endpoint.is_some() {
                close_endpoints = true;
            } else {
                inner.relay_endpoint = Some(relay_endpoint.clone());
                inner.relay_router = Some(relay_router);
                inner.webrtc_endpoint = Some(webrtc_endpoint.clone());
                inner.webrtc_router = Some(webrtc_router);
            }
        }
        if close_endpoints {
            relay_endpoint.close().await;
            webrtc_endpoint.close().await;
        }
        Ok(())
    }

    #[cfg(all(target_family = "wasm", target_os = "unknown"))]
    pub(in crate::browser_worker) async fn restart_webrtc_endpoint(
        &self,
    ) -> BrowserWorkerResult<Endpoint> {
        let (secret_key, transport, low_latency_quic_acks, old_endpoint, old_router) = {
            let mut inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            if inner.closed {
                return Err(BrowserWorkerError::closed());
            }
            (
                inner.secret_key.clone(),
                inner.transport.clone(),
                inner.low_latency_quic_acks,
                inner.webrtc_endpoint.take(),
                inner.webrtc_router.take(),
            )
        };

        if let Some(router) = old_router {
            let _ = router.shutdown().await;
        }
        if let Some(endpoint) = old_endpoint {
            endpoint.close().await;
        }

        let (protocol_transport_lookup, protocol_transport_hook) = {
            let inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            (
                inner.protocol_transport_lookup.clone(),
                self.protocol_transport_hook(),
            )
        };
        let endpoint = bind_webrtc_endpoint(
            secret_key,
            transport,
            low_latency_quic_acks,
            Some(protocol_transport_lookup),
            Some(protocol_transport_hook),
        )
        .await?;
        let mut router = Some(self.build_webrtc_router(endpoint.clone())?);

        let mut close_endpoint = false;
        {
            let mut inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            if inner.closed {
                close_endpoint = true;
            } else {
                inner.webrtc_endpoint = Some(endpoint.clone());
                inner.webrtc_router = router.take();
            }
        }
        if close_endpoint {
            endpoint.close().await;
            if let Some(router) = router {
                let _ = router.shutdown().await;
            }
            return Err(BrowserWorkerError::closed());
        }
        Ok(endpoint)
    }

    #[cfg(test)]
    pub(in crate::browser_worker) fn endpoint_addr(&self) -> BrowserWorkerResult<EndpointAddr> {
        let inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        Ok(inner
            .relay_endpoint
            .as_ref()
            .map(Endpoint::addr)
            .unwrap_or_else(|| {
                EndpointAddr::from_parts(
                    inner.endpoint_id,
                    [TransportAddr::Custom(inner.local_custom_addr.clone())],
                )
            }))
    }

    pub(in crate::browser_worker) fn node_close_result(
        &self,
        reason: Option<String>,
    ) -> WorkerCloseResult {
        let (_, outbound_signals) = self.close_inner(reason);
        WorkerCloseResult {
            closed: true,
            outbound_signals,
        }
    }

    #[cfg(test)]
    pub(in crate::browser_worker) fn close(&self) -> bool {
        self.close_inner(None).0
    }

    fn close_inner(&self, reason: Option<String>) -> (bool, Vec<WebRtcSignal>) {
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return (false, Vec::new());
        }
        inner.closed = true;
        inner.transport.session_hub().close();
        let mut outbound_signals = Vec::new();
        for session in inner.sessions.values_mut() {
            if let Some(signal) = session.close_for_node(reason.clone()) {
                outbound_signals.push(signal);
            }
        }
        for (_, registration) in inner.accepts.drain() {
            registration.complete_waiters_done();
        }
        for connection in inner.connections.values_mut() {
            connection.closed = true;
            if let Some(iroh_connection) = &connection.iroh_connection {
                iroh_connection.close(0u32.into(), b"worker node closed");
            }
        }
        for stream in inner.streams.values() {
            stream.closed_send.store(true, Ordering::SeqCst);
            stream.closed_recv.store(true, Ordering::SeqCst);
            notify_stream_cancel(&stream.send_cancel);
            notify_stream_cancel(&stream.recv_cancel);
            if let Ok(mut send) = stream.send.try_lock() {
                let _ = send.reset(0u32.into());
            }
            if let Ok(mut recv) = stream.recv.try_lock() {
                let _ = recv.stop(0u32.into());
            }
        }
        inner.streams.clear();
        let relay_router = inner.relay_router.take();
        let webrtc_router = inner.webrtc_router.take();
        for (_, abort) in inner.benchmark_echo_tasks.drain() {
            abort.abort();
        }
        if let Some(endpoint) = inner.relay_endpoint.take() {
            n0_future::task::spawn(async move {
                endpoint.close().await;
            });
        }
        if let Some(endpoint) = inner.webrtc_endpoint.take() {
            n0_future::task::spawn(async move {
                endpoint.close().await;
            });
        }
        if let Some(router) = relay_router {
            n0_future::task::spawn(async move {
                let _ = router.shutdown().await;
            });
        }
        if let Some(router) = webrtc_router {
            n0_future::task::spawn(async move {
                let _ = router.shutdown().await;
            });
        }
        (true, outbound_signals)
    }

    pub(in crate::browser_worker) async fn dial_iroh_application_connection(
        &self,
        remote_addr: EndpointAddr,
        alpn: String,
        session_key: Option<WorkerSessionKey>,
    ) -> BrowserWorkerResult<WorkerProtocolConnectionInfo> {
        let endpoint = self.relay_endpoint()?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::connection",
            remote = %remote_addr.id,
            alpn = %alpn,
            session_key = ?session_key.as_ref().map(|key| key.as_str().to_owned()),
            "dialing Iroh relay application connection"
        );
        let connection = endpoint
            .connect(remote_addr, alpn.as_bytes())
            .await
            .map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::connection",
                    alpn = %alpn,
                    %err,
                    "Iroh relay application dial failed"
                );
                if endpoint.is_closed() || self.is_closed() {
                    BrowserWorkerError::closed()
                } else {
                    BrowserWorkerError::new(
                        BrowserWorkerErrorCode::BootstrapFailed,
                        format!("Iroh application dial failed: {err}"),
                    )
                }
            })?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::connection",
            remote = %connection.remote_id(),
            alpn = %String::from_utf8_lossy(connection.alpn()),
            "Iroh relay application dial connected"
        );
        trace_iroh_connection_paths("Iroh relay application dial connected", None, &connection);
        if let Some(session_key) = session_key.as_ref() {
            self.complete_dial(session_key, WorkerResolvedTransport::IrohRelay)?;
        }
        self.admit_iroh_application_connection(
            connection,
            WorkerResolvedTransport::IrohRelay,
            session_key,
            false,
        )
    }

    #[cfg(all(target_family = "wasm", target_os = "unknown"))]
    pub(in crate::browser_worker) async fn dial_webrtc_application_connection(
        &self,
        session_key: &WorkerSessionKey,
    ) -> BrowserWorkerResult<WorkerProtocolConnectionInfo> {
        let session = self.session_snapshot(session_key)?;
        if session.role != WorkerSessionRole::Dialer {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "WebRTC application connection can only be dialed by the dialer session",
            ));
        }
        if session.channel_attachment != WorkerDataChannelAttachmentState::Open {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::DataChannelFailed,
                "WebRTC application connection cannot dial before DataChannel open",
            ));
        }
        let remote_custom_addr =
            WebRtcAddr::session(session.remote, session.dial_id.0).to_custom_addr();
        let remote_addr =
            EndpointAddr::from_parts(session.remote, [TransportAddr::Custom(remote_custom_addr)]);
        let endpoint = self.restart_webrtc_endpoint().await?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::connection",
            session_key = %session_key.as_str(),
            remote = %session.remote,
            alpn = %session.alpn,
            remote_custom_addr = ?remote_addr,
            channel_attachment = ?session.channel_attachment,
            "dialing WebRTC custom transport application connection"
        );
        let connection = endpoint
            .connect(remote_addr, session.alpn.as_bytes())
            .await
            .map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::connection",
                    session_key = %session_key.as_str(),
                    alpn = %session.alpn,
                    %err,
                    "WebRTC custom transport application dial failed"
                );
                if endpoint.is_closed() || self.is_closed() {
                    BrowserWorkerError::closed()
                } else {
                    BrowserWorkerError::new(
                        BrowserWorkerErrorCode::WebRtcFailed,
                        format!("WebRTC application dial failed: {err}"),
                    )
                }
            })?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::connection",
            session_key = %session_key.as_str(),
            remote = %connection.remote_id(),
            alpn = %String::from_utf8_lossy(connection.alpn()),
            "WebRTC custom transport application dial connected"
        );
        trace_iroh_connection_paths(
            "WebRTC custom transport application dial connected",
            None,
            &connection,
        );
        if let Err(err) = require_webrtc_selected_path(&connection, session.remote, session.dial_id)
        {
            connection.close(0u32.into(), b"WebRTC custom path was not selected");
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::connection",
                session_key = %session_key.as_str(),
                %err,
                "rejecting mislabeled WebRTC application connection"
            );
            return Err(err);
        }
        self.complete_dial(session_key, WorkerResolvedTransport::WebRtc)?;
        self.admit_iroh_application_connection(
            connection,
            WorkerResolvedTransport::WebRtc,
            Some(session_key.clone()),
            false,
        )
    }

    pub(in crate::browser_worker) fn relay_endpoint(&self) -> BrowserWorkerResult<Endpoint> {
        let inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        inner.relay_endpoint.clone().ok_or_else(|| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::SpawnFailed,
                "worker relay Iroh endpoint has not been started",
            )
        })
    }

    #[cfg(test)]
    pub(in crate::browser_worker) fn webrtc_endpoint(&self) -> BrowserWorkerResult<Endpoint> {
        let inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        inner.webrtc_endpoint.clone().ok_or_else(|| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::SpawnFailed,
                "worker WebRTC endpoint has not been started",
            )
        })
    }

    fn build_endpoint_routers(
        &self,
        relay_endpoint: Endpoint,
        webrtc_endpoint: Endpoint,
    ) -> BrowserWorkerResult<(Router, Router)> {
        let mut relay_router = Router::builder(relay_endpoint).accept(
            WEBRTC_BOOTSTRAP_ALPN,
            WorkerBootstrapHandler { node: self.clone() },
        );
        let (facade_alpns, worker_protocols) = {
            let inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            (
                inner.accepts.keys().cloned().collect::<Vec<_>>(),
                inner.worker_protocols.clone(),
            )
        };

        for alpn in facade_alpns {
            relay_router = relay_router.accept(
                alpn.as_bytes(),
                WorkerRelayFacadeHandler { node: self.clone() },
            );
        }

        let benchmark_echo_alpns = {
            let inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            inner.benchmark_echo_alpns.clone()
        };
        for alpn in &benchmark_echo_alpns {
            relay_router = relay_router.accept(
                alpn.as_bytes(),
                WorkerBenchmarkEchoHandler {
                    node: self.clone(),
                    transport: WorkerResolvedTransport::IrohRelay,
                },
            );
        }

        let worker_protocol_transport_intent = {
            let inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            inner.worker_protocol_transport_intent
        };
        if !worker_protocol_transport_intent.uses_webrtc() {
            for alpn in worker_protocols.alpns() {
                let relay_handler = worker_protocols
                    .handler(alpn, relay_router.endpoint().clone())
                    .ok_or_else(|| {
                        BrowserWorkerError::new(
                            BrowserWorkerErrorCode::UnsupportedAlpn,
                            format!("missing worker protocol handler for ALPN {alpn:?}"),
                        )
                    })?;
                relay_router = relay_router.accept(alpn.as_bytes(), relay_handler);
            }
        }

        Ok((
            relay_router.spawn(),
            self.build_webrtc_router(webrtc_endpoint)?,
        ))
    }

    fn build_webrtc_router(&self, webrtc_endpoint: Endpoint) -> BrowserWorkerResult<Router> {
        let (facade_alpns, benchmark_echo_alpns, worker_protocols, worker_protocol_transport_intent) = {
            let inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            (
                inner.accepts.keys().cloned().collect::<Vec<_>>(),
                inner.benchmark_echo_alpns.clone(),
                inner.worker_protocols.clone(),
                inner.worker_protocol_transport_intent,
            )
        };

        let mut webrtc_router = Router::builder(webrtc_endpoint);

        for alpn in facade_alpns {
            webrtc_router = webrtc_router.accept(
                alpn.as_bytes(),
                WorkerWebRtcFacadeHandler { node: self.clone() },
            );
        }

        for alpn in benchmark_echo_alpns {
            webrtc_router = webrtc_router.accept(
                alpn.as_bytes(),
                WorkerBenchmarkEchoHandler {
                    node: self.clone(),
                    transport: WorkerResolvedTransport::WebRtc,
                },
            );
        }

        if worker_protocol_transport_intent.uses_webrtc() {
            for alpn in worker_protocols.alpns() {
                let webrtc_handler = worker_protocols
                    .handler(alpn, webrtc_router.endpoint().clone())
                    .ok_or_else(|| {
                        BrowserWorkerError::new(
                            BrowserWorkerErrorCode::UnsupportedAlpn,
                            format!("missing worker protocol handler for ALPN {alpn:?}"),
                        )
                    })?;
                webrtc_router = webrtc_router.accept(
                    alpn.as_bytes(),
                    WorkerWebRtcProtocolGate::new(self.clone(), webrtc_handler),
                );
            }
        }

        Ok(webrtc_router.spawn())
    }

    fn accept_webrtc_connection(
        &self,
        connection: &Connection,
    ) -> BrowserWorkerResult<Option<WorkerSessionKey>> {
        let remote = connection.remote_id();
        let alpn = String::from_utf8_lossy(connection.alpn()).to_string();
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        let Some((session_key, remote, dial_id)) =
            inner.sessions.values_mut().find_map(|session| {
                if session.role == WorkerSessionRole::Acceptor
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
                {
                    Some((session.session_key.clone(), session.remote, session.dial_id))
                } else {
                    None
                }
            })
        else {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "custom app connection did not match WebRTC session",
            ));
        };
        require_webrtc_selected_path(connection, remote, dial_id)?;
        let session = inner
            .sessions
            .get_mut(&session_key)
            .expect("session was selected above");
        validate_transport_resolution(session, WorkerResolvedTransport::WebRtc)?;
        session.resolve_transport(WorkerResolvedTransport::WebRtc)?;
        inner.refresh_webrtc_transport_addrs();
        Ok(Some(session_key))
    }
}

async fn bind_webrtc_endpoint(
    secret_key: SecretKey,
    transport: WebRtcTransport,
    low_latency_quic_acks: bool,
    protocol_transport_lookup: Option<MemoryLookup>,
    protocol_transport_hook: Option<protocol_transport::WorkerProtocolTransportHook>,
) -> BrowserWorkerResult<Endpoint> {
    let mut builder = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(secret_key)
        .clear_relay_transports()
        .clear_address_lookup();
    if let Some(lookup) = protocol_transport_lookup {
        builder = builder.address_lookup(lookup);
    }
    if let Some(hook) = protocol_transport_hook {
        builder = builder.hooks(hook);
    }
    #[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
    {
        builder = builder.clear_ip_transports();
    }
    if low_latency_quic_acks {
        builder = builder.transport_config(low_latency_ack_transport_config());
    }
    crate::transport::configure_endpoint(builder, transport)
        .bind()
        .await
        .map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::SpawnFailed,
                format!("failed to bind worker WebRTC app Iroh endpoint: {err}"),
            )
        })
}

fn initial_accept_registrations(
    alpns: Vec<String>,
    capacity: usize,
) -> HashMap<String, AcceptRegistrationState> {
    let mut next_accept_id = 1;
    let mut accepts = HashMap::new();
    for alpn in alpns {
        let id = WorkerAcceptId(next_accept_id);
        next_accept_id = next_accept_id
            .checked_add(1)
            .expect("worker accept id exhausted");
        accepts.insert(
            alpn.clone(),
            AcceptRegistrationState::new(id, alpn, capacity),
        );
    }
    accepts
}

fn accept_error(error: BrowserWorkerError) -> AcceptError {
    AcceptError::from_err(std::io::Error::new(
        std::io::ErrorKind::Other,
        error.to_string(),
    ))
}

#[derive(Debug, Clone)]
struct WorkerBootstrapHandler {
    node: BrowserWorkerNode,
}

impl ProtocolHandler for WorkerBootstrapHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        if let Some(sender) = self.node.bootstrap_connection_sender() {
            sender.send(connection).map_err(|_| {
                AcceptError::from_err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "worker bootstrap loop not attached",
                ))
            })
        } else {
            connection.close(0u32.into(), b"bootstrap loop not attached");
            Err(AcceptError::from_err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "worker bootstrap loop not attached",
            )))
        }
    }
}

#[derive(Debug, Clone)]
struct WorkerRelayFacadeHandler {
    node: BrowserWorkerNode,
}

impl ProtocolHandler for WorkerRelayFacadeHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        self.node
            .admit_iroh_application_connection(
                connection,
                WorkerResolvedTransport::IrohRelay,
                None,
                true,
            )
            .map(|_| ())
            .map_err(accept_error)
    }
}

#[derive(Debug, Clone)]
struct WorkerWebRtcFacadeHandler {
    node: BrowserWorkerNode,
}

impl ProtocolHandler for WorkerWebRtcFacadeHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let session_key = self
            .node
            .accept_webrtc_connection(&connection)
            .map_err(|err| {
                connection.close(
                    0u32.into(),
                    b"custom app connection did not match WebRTC session",
                );
                accept_error(err)
            })?;
        self.node
            .admit_iroh_application_connection(
                connection,
                WorkerResolvedTransport::WebRtc,
                session_key,
                true,
            )
            .map(|_| ())
            .map_err(accept_error)
    }
}

#[derive(Debug, Clone)]
struct WorkerBenchmarkEchoHandler {
    node: BrowserWorkerNode,
    transport: WorkerResolvedTransport,
}

impl ProtocolHandler for WorkerBenchmarkEchoHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let session_key = if self.transport == WorkerResolvedTransport::WebRtc {
            self.node
                .accept_webrtc_connection(&connection)
                .map_err(|err| {
                    connection.close(
                        0u32.into(),
                        b"custom app connection did not match WebRTC session",
                    );
                    accept_error(err)
                })?
        } else {
            None
        };
        let (connection_key, _) = self
            .node
            .admit_iroh_application_connection_with_key(
                connection,
                self.transport,
                session_key,
                false,
            )
            .map_err(accept_error)?;
        let node = self.node.clone();
        n0_future::task::spawn(async move {
            if let Err(err) = node.run_benchmark_echo_connection(connection_key).await {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::benchmark",
                    connection_key = connection_key.0,
                    %err,
                    "benchmark echo connection failed"
                );
            }
        });
        Ok(())
    }
}

#[derive(Debug)]
struct WorkerWebRtcProtocolGate {
    node: BrowserWorkerNode,
    handler: Box<dyn DynProtocolHandler>,
}

impl WorkerWebRtcProtocolGate {
    fn new(node: BrowserWorkerNode, handler: Box<dyn DynProtocolHandler>) -> Self {
        Self { node, handler }
    }
}

impl ProtocolHandler for WorkerWebRtcProtocolGate {
    async fn on_accepting(
        &self,
        accepting: iroh::endpoint::Accepting,
    ) -> std::result::Result<Connection, AcceptError> {
        let connection = self.handler.on_accepting(accepting).await?;
        self.node
            .accept_webrtc_connection(&connection)
            .map_err(|err| {
                connection.close(
                    0u32.into(),
                    b"custom app connection did not match WebRTC session",
                );
                accept_error(err)
            })?;
        Ok(connection)
    }

    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        self.handler.accept(connection).await
    }

    async fn shutdown(&self) {
        self.handler.shutdown().await;
    }
}

fn low_latency_ack_transport_config() -> QuicTransportConfig {
    let mut ack_frequency = AckFrequencyConfig::default();
    ack_frequency
        .ack_eliciting_threshold(VarInt::from_u32(0))
        .max_ack_delay(Some(Duration::from_millis(1)));

    QuicTransportConfig::builder()
        .ack_frequency_config(Some(ack_frequency))
        .build()
}
