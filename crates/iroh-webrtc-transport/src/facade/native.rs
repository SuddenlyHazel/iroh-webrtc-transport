use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};

use anyhow::{Context, Result, bail};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr,
    endpoint::{Builder, Connection, Incoming, presets},
};
use n0_future::task::{self, JoinHandle};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

use crate::{
    core::{
        addr::WebRtcAddr,
        bootstrap::{BootstrapConfig, BootstrapSignalReceiver, BootstrapSignalSender},
        bootstrap::{BootstrapStream, WEBRTC_BOOTSTRAP_ALPN},
        coordinator::DialCoordinator,
        hub::WebRtcIceCandidate,
        signaling::{
            BootstrapTransportIntent, DialIdGenerator, WEBRTC_DATA_CHANNEL_LABEL, WebRtcSignal,
            WebRtcTerminalReason,
        },
    },
    facade::{WebRtcDialOptions, WebRtcNodeConfig},
    native::{LocalIceEvent, NativeWebRtcSession},
    transport::{WebRtcTransport, configure_endpoint},
};

/// Native application facade that owns an Iroh endpoint plus WebRTC bootstrap.
///
/// This type is only available with the `native` feature on non-wasm targets.
/// It returns ordinary Iroh [`Connection`] values for application protocols.
#[derive(Debug, Clone)]
pub struct NativeWebRtcIrohNode {
    inner: Arc<NativeNodeInner>,
}

#[derive(Debug)]
struct NativeNodeInner {
    secret_key: SecretKey,
    endpoint: Endpoint,
    webrtc_endpoint: StdMutex<Endpoint>,
    transport: WebRtcTransport,
    config: WebRtcNodeConfig,
    state: StdMutex<NativeNodeState>,
}

#[derive(Debug)]
struct NativeNodeState {
    closed: bool,
    accept_abort: Option<task::AbortHandle>,
    webrtc_accept_abort: Option<task::AbortHandle>,
    dial_ids: DialIdGenerator,
    accept_queues: HashMap<Vec<u8>, NativeAcceptQueue>,
    sessions: Vec<NativeWebRtcSession>,
}

#[derive(Debug)]
struct NativeAcceptQueue {
    tx: mpsc::Sender<Connection>,
    rx: Arc<AsyncMutex<mpsc::Receiver<Connection>>>,
}

impl NativeWebRtcIrohNode {
    /// Binds a native facade node with the default Iroh endpoint preset.
    pub async fn bind(config: WebRtcNodeConfig, secret_key: SecretKey) -> Result<Self> {
        let builder = Endpoint::builder(presets::N0);
        Self::bind_with_builder(config, secret_key, builder).await
    }

    /// Alias for [`Self::bind`], matching runtimes that name endpoint startup as spawn.
    pub async fn spawn(config: WebRtcNodeConfig, secret_key: SecretKey) -> Result<Self> {
        Self::bind(config, secret_key).await
    }

    /// Binds a native facade node from a caller-supplied Iroh endpoint builder.
    ///
    /// The facade installs the WebRTC custom transport and manages the accepted
    /// ALPN list. Builder ALPNs are replaced by the facade.
    pub async fn bind_with_builder(
        config: WebRtcNodeConfig,
        secret_key: SecretKey,
        builder: Builder,
    ) -> Result<Self> {
        config
            .validate()
            .context("invalid native WebRTC node config")?;
        let transport = WebRtcTransport::try_new(config.transport.clone())
            .context("invalid native WebRTC transport config")?;
        let endpoint = builder
            .secret_key(secret_key.clone())
            .alpns(vec![WEBRTC_BOOTSTRAP_ALPN.to_vec()])
            .bind()
            .await
            .context("failed to bind native WebRTC bootstrap endpoint")?;
        let webrtc_endpoint =
            bind_webrtc_application_endpoint(secret_key.clone(), Vec::new(), transport.clone())
                .await
                .context("failed to bind native WebRTC application endpoint")?;
        let endpoint_id = endpoint.id();
        let node = Self {
            inner: Arc::new(NativeNodeInner {
                secret_key,
                endpoint,
                webrtc_endpoint: StdMutex::new(webrtc_endpoint),
                transport,
                config,
                state: StdMutex::new(NativeNodeState {
                    closed: false,
                    accept_abort: None,
                    webrtc_accept_abort: None,
                    dial_ids: DialIdGenerator::new(endpoint_id),
                    accept_queues: HashMap::new(),
                    sessions: Vec::new(),
                }),
            }),
        };
        let accept_node = node.clone();
        let accept = task::spawn(async move {
            accept_node.run_accept_loop().await;
        });
        let webrtc_accept_node = node.clone();
        let webrtc_accept = task::spawn(async move {
            webrtc_accept_node.run_webrtc_accept_loop().await;
        });
        {
            let mut state = node
                .inner
                .state
                .lock()
                .expect("native facade node mutex poisoned");
            state.accept_abort = Some(accept.abort_handle());
            state.webrtc_accept_abort = Some(webrtc_accept.abort_handle());
        }
        Ok(node)
    }

    /// Returns the local Iroh endpoint id.
    pub fn endpoint_id(&self) -> EndpointId {
        self.inner.endpoint.id()
    }

    /// Returns the current Iroh endpoint address.
    pub fn endpoint_addr(&self) -> EndpointAddr {
        self.inner.endpoint.addr()
    }

    /// Returns a clone of the underlying Iroh endpoint.
    pub fn endpoint(&self) -> Endpoint {
        self.inner.endpoint.clone()
    }

    /// Returns the local WebRTC capability address.
    pub fn webrtc_capability_addr(&self) -> WebRtcAddr {
        WebRtcAddr::capability(self.endpoint_id())
    }

    /// Accepts one application connection for `alpn`.
    pub async fn accept(&self, alpn: impl AsRef<[u8]>) -> Result<Connection> {
        let alpn = alpn.as_ref().to_vec();
        let rx = self.accept_receiver(alpn)?;
        let mut rx = rx.lock().await;
        let connection = rx
            .recv()
            .await
            .context("native WebRTC accept queue closed")?;
        Ok(connection)
    }

    /// Dials an application protocol and returns the resulting Iroh connection.
    ///
    /// For WebRTC intents this performs the native bootstrap exchange first and
    /// then dials the session-scoped custom transport address. For relay-only
    /// intents this delegates directly to the underlying Iroh endpoint.
    pub async fn dial(
        &self,
        remote: EndpointAddr,
        alpn: impl AsRef<[u8]>,
        options: WebRtcDialOptions,
    ) -> Result<Connection> {
        let alpn = alpn.as_ref().to_vec();
        if !options.transport_intent.uses_webrtc() {
            return self.dial_iroh(remote, alpn).await;
        }

        match self
            .dial_webrtc(remote.clone(), alpn.clone(), options)
            .await
        {
            Ok(connection) => Ok(connection),
            Err(err)
                if options.transport_intent.allows_iroh_relay_fallback()
                    && !self.inner.endpoint.is_closed() =>
            {
                tracing::debug!(
                    error = %err,
                    remote = %remote.id,
                    "native WebRTC dial failed; attempting Iroh fallback"
                );
                self.dial_iroh(remote, alpn).await
            }
            Err(err) => Err(err),
        }
    }

    /// Closes the endpoint, accept loop, and active native WebRTC sessions.
    pub async fn close(&self) {
        let (accept_abort, webrtc_accept_abort, sessions) = {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("native facade node mutex poisoned");
            if state.closed {
                return;
            }
            state.closed = true;
            let accept_abort = state.accept_abort.take();
            let webrtc_accept_abort = state.webrtc_accept_abort.take();
            state.accept_queues.clear();
            let sessions = std::mem::take(&mut state.sessions);
            (accept_abort, webrtc_accept_abort, sessions)
        };
        if let Some(accept_abort) = accept_abort {
            accept_abort.abort();
        }
        if let Some(accept_abort) = webrtc_accept_abort {
            accept_abort.abort();
        }
        self.inner.endpoint.close().await;
        self.webrtc_endpoint().close().await;
        for session in sessions {
            session.close().await;
        }
    }

    async fn dial_iroh(&self, remote: EndpointAddr, alpn: Vec<u8>) -> Result<Connection> {
        self.inner
            .endpoint
            .connect(remote, &alpn)
            .await
            .context("failed to dial Iroh application connection")
    }

    async fn dial_webrtc(
        &self,
        remote: EndpointAddr,
        alpn: Vec<u8>,
        options: WebRtcDialOptions,
    ) -> Result<Connection> {
        let remote_id = remote.id;
        let alpn_string = String::from_utf8(alpn.clone())
            .context("native WebRTC bootstrap ALPN must be valid UTF-8")?;
        let coordinator = self.allocate_dial(remote_id, options.transport_intent)?;
        let remote_session_addr =
            WebRtcAddr::session(remote_id, coordinator.dial_id().0).to_custom_addr();
        let session = NativeWebRtcSession::new_offerer_with_config(
            self.inner.transport.session_hub(),
            remote_session_addr.clone(),
            coordinator.dial_id().0,
            WEBRTC_DATA_CHANNEL_LABEL,
            self.inner.config.session.clone(),
        )
        .await
        .context("failed to create native WebRTC offerer")?;
        self.track_session(session.clone())?;

        let bootstrap =
            BootstrapStream::connect(&self.inner.endpoint, remote, BootstrapConfig::default())
                .await
                .context("failed to connect native WebRTC bootstrap stream")?;
        let channel = bootstrap
            .open_channel()
            .await
            .context("failed to open native WebRTC bootstrap channel")?;
        let (mut sender, receiver) = channel.split();
        sender
            .send_signal(&WebRtcSignal::dial_request_with_alpn(
                coordinator.dial_id(),
                self.endpoint_id(),
                remote_id,
                coordinator.generation(),
                alpn_string,
                options.transport_intent,
            ))
            .await
            .context("failed to send native WebRTC dial request")?;
        let offer = session
            .create_offer()
            .await
            .context("failed to create native WebRTC offer")?;
        sender
            .send_signal(&coordinator.offer(offer))
            .await
            .context("failed to send native WebRTC offer")?;
        let ice = spawn_local_ice_sender(session.clone(), coordinator.clone(), sender);

        receive_dialer_signals(session, coordinator.clone(), receiver)
            .await
            .context("failed while receiving native WebRTC answerer signals")?;
        let _ = ice.await;

        let expected_remote_addr = WebRtcAddr::session(remote_id, coordinator.dial_id().0);
        let application_addr =
            EndpointAddr::from_parts(remote_id, [TransportAddr::Custom(remote_session_addr)]);
        let webrtc_endpoint = self.restart_webrtc_application_endpoint().await?;
        let connection = webrtc_endpoint
            .connect(application_addr, &alpn)
            .await
            .context("failed to dial WebRTC custom transport application connection")?;
        if let Err(err) = require_webrtc_selected_path(&connection, expected_remote_addr) {
            connection.close(0u32.into(), b"WebRTC custom path was not selected");
            return Err(err).context("WebRTC application dial selected the wrong transport path");
        }
        Ok(connection)
    }

    fn allocate_dial(
        &self,
        remote: EndpointId,
        transport_intent: BootstrapTransportIntent,
    ) -> Result<DialCoordinator> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("native facade node mutex poisoned");
        if state.closed {
            bail!("native WebRTC node is closed");
        }
        let coordinator = DialCoordinator::initiate_with_transport_intent(
            &mut state.dial_ids,
            remote,
            transport_intent,
        );
        self.inner.transport.advertise_local_addr(
            WebRtcAddr::session(self.endpoint_id(), coordinator.dial_id().0).to_custom_addr(),
        );
        Ok(coordinator)
    }

    fn track_session(&self, session: NativeWebRtcSession) -> Result<()> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("native facade node mutex poisoned");
        if state.closed {
            bail!("native WebRTC node is closed");
        }
        state.sessions.push(session);
        Ok(())
    }

    fn accept_receiver(
        &self,
        alpn: Vec<u8>,
    ) -> Result<Arc<AsyncMutex<mpsc::Receiver<Connection>>>> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("native facade node mutex poisoned");
        if state.closed {
            bail!("native WebRTC node is closed");
        }
        let receiver = state
            .accept_queues
            .entry(alpn)
            .or_insert_with(|| {
                let (tx, rx) = mpsc::channel(16);
                NativeAcceptQueue {
                    tx,
                    rx: Arc::new(AsyncMutex::new(rx)),
                }
            })
            .rx
            .clone();
        refresh_bootstrap_alpns(&self.inner.endpoint, &state.accept_queues);
        refresh_webrtc_alpns(&self.webrtc_endpoint(), &state.accept_queues);
        Ok(receiver)
    }

    async fn run_accept_loop(&self) {
        while let Some(incoming) = self.inner.endpoint.accept().await {
            let node = self.clone();
            task::spawn(async move {
                node.handle_incoming(incoming).await;
            });
        }
    }

    async fn run_webrtc_accept_loop(&self) {
        let endpoint = self.webrtc_endpoint();
        while let Some(incoming) = endpoint.accept().await {
            let node = self.clone();
            task::spawn(async move {
                node.handle_webrtc_incoming(incoming).await;
            });
        }
    }

    fn webrtc_endpoint(&self) -> Endpoint {
        self.inner
            .webrtc_endpoint
            .lock()
            .expect("native facade WebRTC endpoint mutex poisoned")
            .clone()
    }

    async fn restart_webrtc_application_endpoint(&self) -> Result<Endpoint> {
        let (alpns, old_endpoint, old_accept_abort) = {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("native facade node mutex poisoned");
            if state.closed {
                bail!("native WebRTC node is closed");
            }
            let alpns = state.accept_queues.keys().cloned().collect();
            let old_accept_abort = state.webrtc_accept_abort.take();
            let old_endpoint = self.webrtc_endpoint();
            (alpns, old_endpoint, old_accept_abort)
        };
        if let Some(abort) = old_accept_abort {
            abort.abort();
        }
        old_endpoint.close().await;

        let endpoint = bind_webrtc_application_endpoint(
            self.inner.secret_key.clone(),
            alpns,
            self.inner.transport.clone(),
        )
        .await?;
        {
            let mut current = self
                .inner
                .webrtc_endpoint
                .lock()
                .expect("native facade WebRTC endpoint mutex poisoned");
            *current = endpoint.clone();
        }

        let accept_node = self.clone();
        let accept = task::spawn(async move {
            accept_node.run_webrtc_accept_loop().await;
        });
        let node_closed = {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("native facade node mutex poisoned");
            if state.closed {
                true
            } else {
                state.webrtc_accept_abort = Some(accept.abort_handle());
                false
            }
        };
        if node_closed {
            accept.abort();
            endpoint.close().await;
            bail!("native WebRTC node is closed");
        }
        Ok(endpoint)
    }

    async fn handle_incoming(&self, incoming: Incoming) {
        let connection = match incoming.await {
            Ok(connection) => connection,
            Err(err) => {
                tracing::debug!(%err, "native facade incoming handshake failed");
                return;
            }
        };
        if connection.alpn() == WEBRTC_BOOTSTRAP_ALPN {
            let node = self.clone();
            task::spawn(async move {
                if let Err(err) = node.handle_bootstrap_connection(connection).await {
                    tracing::debug!(%err, "native WebRTC bootstrap connection failed");
                }
            });
            return;
        }

        self.queue_accepted_connection(connection).await;
    }

    async fn handle_webrtc_incoming(&self, incoming: Incoming) {
        let connection = match incoming.await {
            Ok(connection) => connection,
            Err(err) => {
                tracing::debug!(%err, "native facade incoming WebRTC handshake failed");
                return;
            }
        };
        if connection.alpn() == WEBRTC_BOOTSTRAP_ALPN {
            connection.close(
                0u32.into(),
                b"bootstrap is not accepted on WebRTC app endpoint",
            );
            return;
        }
        self.queue_accepted_connection(connection).await;
    }

    async fn queue_accepted_connection(&self, connection: Connection) {
        let alpn = connection.alpn().to_vec();
        let tx = {
            let state = self
                .inner
                .state
                .lock()
                .expect("native facade node mutex poisoned");
            state.accept_queues.get(&alpn).map(|queue| queue.tx.clone())
        };
        match tx {
            Some(tx) if tx.send(connection.clone()).await.is_ok() => {}
            _ => connection.close(0u32.into(), b"native facade ALPN is not accepting"),
        }
    }

    async fn handle_bootstrap_connection(&self, connection: Connection) -> Result<()> {
        let bootstrap = BootstrapStream::from_connection(connection, BootstrapConfig::default());
        let channel = bootstrap
            .accept_channel()
            .await
            .context("failed to accept native WebRTC bootstrap channel")?;
        let (sender, mut receiver) = channel.split();
        let request = receiver
            .recv_signal()
            .await
            .context("failed to receive native WebRTC dial request")?;
        let WebRtcSignal::DialRequest {
            dial_id,
            from,
            to,
            generation,
            alpn,
            transport_intent,
        } = request
        else {
            bail!("first native WebRTC bootstrap signal was not a dial request");
        };
        if to != self.endpoint_id() {
            bail!("native WebRTC bootstrap request was not addressed to this endpoint");
        }
        if !transport_intent.uses_webrtc() {
            return Ok(());
        }
        self.ensure_accepting_alpn(alpn.as_bytes())?;

        let coordinator = DialCoordinator::respond_with_transport_intent(
            self.endpoint_id(),
            from,
            generation,
            dial_id,
            transport_intent,
        );
        self.inner.transport.advertise_local_addr(
            WebRtcAddr::session(self.endpoint_id(), coordinator.dial_id().0).to_custom_addr(),
        );
        let remote_session_addr =
            WebRtcAddr::session(from, coordinator.dial_id().0).to_custom_addr();
        let session = NativeWebRtcSession::new_answerer_with_config(
            self.inner.transport.session_hub(),
            remote_session_addr,
            coordinator.dial_id().0,
            self.inner.config.session.clone(),
        )
        .await
        .context("failed to create native WebRTC answerer")?;
        self.track_session(session.clone())?;

        receive_answerer_signals(session, coordinator, sender, receiver)
            .await
            .context("failed while receiving native WebRTC offerer signals")
    }

    fn ensure_accepting_alpn(&self, alpn: &[u8]) -> Result<()> {
        let state = self
            .inner
            .state
            .lock()
            .expect("native facade node mutex poisoned");
        if state.accept_queues.contains_key(alpn) {
            Ok(())
        } else {
            bail!("no active native WebRTC accept registration for ALPN")
        }
    }
}

fn refresh_bootstrap_alpns(
    endpoint: &Endpoint,
    accept_queues: &HashMap<Vec<u8>, NativeAcceptQueue>,
) {
    let mut alpns = Vec::with_capacity(accept_queues.len() + 1);
    alpns.push(WEBRTC_BOOTSTRAP_ALPN.to_vec());
    alpns.extend(accept_queues.keys().cloned());
    endpoint.set_alpns(alpns);
}

fn refresh_webrtc_alpns(endpoint: &Endpoint, accept_queues: &HashMap<Vec<u8>, NativeAcceptQueue>) {
    endpoint.set_alpns(accept_queues.keys().cloned().collect());
}

async fn bind_webrtc_application_endpoint(
    secret_key: SecretKey,
    alpns: Vec<Vec<u8>>,
    transport: WebRtcTransport,
) -> Result<Endpoint> {
    let builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret_key)
        .alpns(alpns)
        .clear_relay_transports()
        .clear_address_lookup()
        .clear_ip_transports();
    configure_endpoint(builder, transport)
        .bind()
        .await
        .context("failed to bind native WebRTC application endpoint")
}

fn require_webrtc_selected_path(connection: &Connection, expected: WebRtcAddr) -> Result<()> {
    for path in connection.paths().into_iter() {
        if !path.is_selected() {
            continue;
        }
        let TransportAddr::Custom(custom_addr) = path.remote_addr() else {
            bail!(
                "WebRTC application connection selected non-custom Iroh path: {:?}",
                path.remote_addr()
            );
        };
        let actual = WebRtcAddr::from_custom_addr(custom_addr)
            .context("selected custom path is not a WebRTC session address")?;
        if actual == expected {
            return Ok(());
        }
        bail!(
            "selected WebRTC custom path does not match session: expected {:?}, got {:?}",
            expected,
            actual
        );
    }

    bail!("WebRTC application connection has no selected Iroh path")
}

async fn receive_dialer_signals(
    session: NativeWebRtcSession,
    coordinator: DialCoordinator,
    mut receiver: BootstrapSignalReceiver,
) -> Result<()> {
    let mut answered = false;
    loop {
        let signal = receiver.recv_signal().await?;
        ensure_signal_route(&coordinator, &signal)?;
        match signal {
            WebRtcSignal::Answer { sdp, .. } => {
                session.apply_answer(sdp).await?;
                answered = true;
            }
            WebRtcSignal::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                ..
            } => {
                session
                    .add_ice_candidate(WebRtcIceCandidate {
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                    })
                    .await?;
            }
            WebRtcSignal::EndOfCandidates { .. } => {
                session.add_end_of_candidates().await?;
                if answered {
                    return Ok(());
                }
            }
            WebRtcSignal::Terminal {
                reason, message, ..
            } => return Err(terminal_error(reason, message)),
            _ => bail!("unexpected native WebRTC answerer signal"),
        }
    }
}

async fn receive_answerer_signals(
    session: NativeWebRtcSession,
    coordinator: DialCoordinator,
    sender: BootstrapSignalSender,
    mut receiver: BootstrapSignalReceiver,
) -> Result<()> {
    let mut offered = false;
    let mut sender = Some(sender);
    let mut ice = None;
    loop {
        let signal = receiver.recv_signal().await?;
        ensure_signal_route(&coordinator, &signal)?;
        match signal {
            WebRtcSignal::Offer { sdp, .. } => {
                session.apply_offer(sdp).await?;
                let answer = session.create_answer().await?;
                let mut sender = sender
                    .take()
                    .context("native WebRTC answer was already sent for this session")?;
                sender.send_signal(&coordinator.answer(answer)).await?;
                ice = Some(spawn_local_ice_sender(
                    session.clone(),
                    coordinator.clone(),
                    sender,
                ));
                offered = true;
            }
            WebRtcSignal::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
                ..
            } => {
                session
                    .add_ice_candidate(WebRtcIceCandidate {
                        candidate,
                        sdp_mid,
                        sdp_mline_index,
                    })
                    .await?;
            }
            WebRtcSignal::EndOfCandidates { .. } => {
                session.add_end_of_candidates().await?;
                if offered {
                    if let Some(ice) = ice.take() {
                        let _ = ice.await;
                    }
                    return Ok(());
                }
            }
            WebRtcSignal::Terminal {
                reason, message, ..
            } => return Err(terminal_error(reason, message)),
            _ => bail!("unexpected native WebRTC offerer signal"),
        }
    }
}

fn spawn_local_ice_sender(
    session: NativeWebRtcSession,
    coordinator: DialCoordinator,
    mut sender: BootstrapSignalSender,
) -> JoinHandle<()> {
    task::spawn(async move {
        loop {
            let signal = match session.next_local_ice().await {
                Ok(LocalIceEvent::Candidate(candidate)) => coordinator.ice_candidate(candidate),
                Ok(LocalIceEvent::EndOfCandidates) => coordinator.end_of_candidates(),
                Err(err) => {
                    tracing::debug!(%err, "native WebRTC local ICE collection stopped");
                    return;
                }
            };
            let terminal = matches!(signal, WebRtcSignal::EndOfCandidates { .. });
            if let Err(err) = sender.send_signal(&signal).await {
                tracing::debug!(%err, "failed to send native WebRTC local ICE signal");
                return;
            }
            if terminal {
                let _ = sender.finish();
                return;
            }
        }
    })
}

fn ensure_signal_route(coordinator: &DialCoordinator, signal: &WebRtcSignal) -> Result<()> {
    if coordinator.accepts(signal) {
        Ok(())
    } else {
        bail!("native WebRTC bootstrap signal route did not match the session")
    }
}

fn terminal_error(reason: WebRtcTerminalReason, message: Option<String>) -> anyhow::Error {
    let message = message.unwrap_or_else(|| "remote terminal WebRTC signal".to_owned());
    match reason {
        WebRtcTerminalReason::WebRtcFailed => anyhow::anyhow!("remote WebRTC failure: {message}"),
        WebRtcTerminalReason::FallbackSelected => {
            anyhow::anyhow!("remote selected WebRTC fallback: {message}")
        }
        WebRtcTerminalReason::Cancelled => {
            anyhow::anyhow!("remote cancelled WebRTC bootstrap: {message}")
        }
        WebRtcTerminalReason::Closed => {
            anyhow::anyhow!("remote closed WebRTC bootstrap: {message}")
        }
    }
}
