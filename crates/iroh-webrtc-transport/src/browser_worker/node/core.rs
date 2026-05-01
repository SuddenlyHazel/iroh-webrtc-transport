use super::*;
use iroh::endpoint::{AckFrequencyConfig, QuicTransportConfig, VarInt};
use std::time::Duration;

impl BrowserWorkerNode {
    pub(in crate::browser_worker) fn spawn(
        config: BrowserWorkerNodeConfig,
    ) -> BrowserWorkerResult<Self> {
        Self::spawn_with_secret_key(config, SecretKey::generate())
    }

    #[cfg(test)]
    pub(in crate::browser_worker) fn spawn_default() -> BrowserWorkerResult<Self> {
        Self::spawn(BrowserWorkerNodeConfig::default())
    }

    pub(in crate::browser_worker) fn spawn_with_secret_key(
        config: BrowserWorkerNodeConfig,
        secret_key: SecretKey,
    ) -> BrowserWorkerResult<Self> {
        config.validate()?;
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

        Ok(Self {
            inner: Arc::new(Mutex::new(BrowserWorkerNodeInner {
                secret_key,
                node_key: WorkerNodeKey(format!("node-{}", endpoint_id)),
                endpoint_id,
                local_custom_addr,
                transport,
                relay_endpoint: None,
                relay_accept_abort: None,
                webrtc_endpoint: None,
                webrtc_accept_abort: None,
                benchmark_echo_tasks: HashMap::new(),
                bootstrap_connection_tx: None,
                dial_ids: DialIdGenerator::new(endpoint_id),
                accept_queue_capacity: config.accept_queue_capacity,
                session_config: config.session_config,
                low_latency_quic_acks: config.low_latency_quic_acks,
                sessions: HashMap::new(),
                connections: HashMap::new(),
                streams: HashMap::new(),
                accepts: HashMap::new(),
                next_accept_id: 1,
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
        let (secret_key, transport, relay_alpns, webrtc_alpns, low_latency_quic_acks) = {
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
                inner.relay_endpoint_alpns(),
                inner.webrtc_endpoint_alpns(),
                inner.low_latency_quic_acks,
            )
        };

        let mut relay_builder = Endpoint::builder(iroh::endpoint::presets::N0)
            .secret_key(secret_key.clone())
            .alpns(relay_alpns);
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
        let webrtc_endpoint =
            bind_webrtc_endpoint(secret_key, webrtc_alpns, transport, low_latency_quic_acks)
                .await?;
        let relay_accept_abort = self.spawn_relay_endpoint_accept_loop(relay_endpoint.clone());
        let webrtc_accept_abort = self.spawn_webrtc_endpoint_accept_loop(webrtc_endpoint.clone());

        let mut close_endpoints = false;
        {
            let mut inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            if inner.closed || inner.relay_endpoint.is_some() || inner.webrtc_endpoint.is_some() {
                close_endpoints = true;
                relay_accept_abort.abort();
                webrtc_accept_abort.abort();
            } else {
                inner.relay_endpoint = Some(relay_endpoint.clone());
                inner.relay_accept_abort = Some(relay_accept_abort);
                inner.webrtc_endpoint = Some(webrtc_endpoint.clone());
                inner.webrtc_accept_abort = Some(webrtc_accept_abort);
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
        let (secret_key, transport, webrtc_alpns, low_latency_quic_acks, old_endpoint) = {
            let mut inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            if inner.closed {
                return Err(BrowserWorkerError::closed());
            }
            if let Some(abort) = inner.webrtc_accept_abort.take() {
                abort.abort();
            }
            (
                inner.secret_key.clone(),
                inner.transport.clone(),
                inner.webrtc_endpoint_alpns(),
                inner.low_latency_quic_acks,
                inner.webrtc_endpoint.take(),
            )
        };

        if let Some(endpoint) = old_endpoint {
            endpoint.close().await;
        }

        let endpoint =
            bind_webrtc_endpoint(secret_key, webrtc_alpns, transport, low_latency_quic_acks)
                .await?;
        let accept_abort = self.spawn_webrtc_endpoint_accept_loop(endpoint.clone());

        let mut close_endpoint = false;
        {
            let mut inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            if inner.closed {
                close_endpoint = true;
                accept_abort.abort();
            } else {
                inner.webrtc_endpoint = Some(endpoint.clone());
                inner.webrtc_accept_abort = Some(accept_abort);
            }
        }
        if close_endpoint {
            endpoint.close().await;
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
        if let Some(abort) = inner.relay_accept_abort.take() {
            abort.abort();
        }
        if let Some(abort) = inner.webrtc_accept_abort.take() {
            abort.abort();
        }
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
        (true, outbound_signals)
    }

    fn spawn_relay_endpoint_accept_loop(&self, endpoint: Endpoint) -> n0_future::task::AbortHandle {
        let node = self.clone();
        let handle = n0_future::task::spawn(async move {
            node.run_relay_endpoint_accept_loop(endpoint).await;
        });
        handle.abort_handle()
    }

    fn spawn_webrtc_endpoint_accept_loop(
        &self,
        endpoint: Endpoint,
    ) -> n0_future::task::AbortHandle {
        let node = self.clone();
        let handle = n0_future::task::spawn(async move {
            node.run_webrtc_endpoint_accept_loop(endpoint).await;
        });
        handle.abort_handle()
    }

    async fn run_relay_endpoint_accept_loop(&self, endpoint: Endpoint) {
        loop {
            let Some(incoming) = endpoint.accept().await else {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::connection",
                    "relay endpoint accept loop ended"
                );
                return;
            };
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::connection",
                "endpoint accepted incoming handshake"
            );
            let connection = match incoming.await {
                Ok(connection) => connection,
                Err(err) => {
                    tracing::debug!(
                        target: "iroh_webrtc_transport::browser_worker::connection",
                        %err,
                        "incoming handshake failed"
                    );
                    continue;
                }
            };
            let alpn = String::from_utf8_lossy(connection.alpn()).to_string();
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::connection",
                remote = %connection.remote_id(),
                alpn = %alpn,
                "accepted Iroh connection"
            );
            trace_iroh_connection_paths("accepted relay endpoint handshake", None, &connection);
            if alpn == bootstrap_alpn_str() {
                if let Some(sender) = self.bootstrap_connection_sender() {
                    if sender.send(connection).is_err() {
                        // The Wasm bootstrap driver is gone; close the relay
                        // bootstrap connection instead of leaving it pending.
                    }
                } else {
                    connection.close(0u32.into(), b"bootstrap loop not attached");
                }
                continue;
            }
            if self
                .admit_iroh_application_connection(
                    connection,
                    WorkerResolvedTransport::IrohRelay,
                    None,
                    true,
                )
                .is_err()
            {
                // The ALPN may have been closed after the endpoint accepted the handshake.
                // Closing the connection keeps the worker's accept gate authoritative.
            }
        }
    }

    async fn run_webrtc_endpoint_accept_loop(&self, endpoint: Endpoint) {
        loop {
            let Some(incoming) = endpoint.accept().await else {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::connection",
                    "WebRTC endpoint accept loop ended"
                );
                return;
            };
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::connection",
                "WebRTC endpoint accepted incoming handshake"
            );
            let connection = match incoming.await {
                Ok(connection) => connection,
                Err(err) => {
                    tracing::debug!(
                        target: "iroh_webrtc_transport::browser_worker::connection",
                        %err,
                        "incoming WebRTC endpoint handshake failed"
                    );
                    continue;
                }
            };
            let alpn = String::from_utf8_lossy(connection.alpn()).to_string();
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::connection",
                remote = %connection.remote_id(),
                alpn = %alpn,
                "accepted WebRTC endpoint Iroh connection"
            );
            trace_iroh_connection_paths("accepted WebRTC endpoint handshake", None, &connection);
            if alpn == bootstrap_alpn_str() {
                connection.close(
                    0u32.into(),
                    b"bootstrap is not accepted on WebRTC app endpoint",
                );
                continue;
            }
            let (transport, session_key) =
                self.incoming_application_route(connection.remote_id(), &alpn);
            if transport != WorkerResolvedTransport::WebRtc {
                connection.close(
                    0u32.into(),
                    b"custom app connection did not match WebRTC session",
                );
                continue;
            }
            if self
                .admit_iroh_application_connection(connection, transport, session_key, true)
                .is_err()
            {
                // The ALPN/session may have been closed after the endpoint accepted the handshake.
            }
        }
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
}

async fn bind_webrtc_endpoint(
    secret_key: SecretKey,
    alpns: Vec<Vec<u8>>,
    transport: WebRtcTransport,
    low_latency_quic_acks: bool,
) -> BrowserWorkerResult<Endpoint> {
    let mut builder = Endpoint::builder(iroh::endpoint::presets::Minimal)
        .secret_key(secret_key)
        .alpns(alpns)
        .clear_relay_transports()
        .clear_address_lookup();
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

fn low_latency_ack_transport_config() -> QuicTransportConfig {
    let mut ack_frequency = AckFrequencyConfig::default();
    ack_frequency
        .ack_eliciting_threshold(VarInt::from_u32(0))
        .max_ack_delay(Some(Duration::from_millis(1)));

    QuicTransportConfig::builder()
        .ack_frequency_config(Some(ack_frequency))
        .build()
}
