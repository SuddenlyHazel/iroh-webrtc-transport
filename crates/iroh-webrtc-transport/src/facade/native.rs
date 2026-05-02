use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};

use anyhow::{Context, Result, bail};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr,
    endpoint::{Accepting, Builder, Connection, presets},
    protocol::{AcceptError, DynProtocolHandler, ProtocolHandler, Router},
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
            BootstrapTransportIntent, DialId, DialIdGenerator, WEBRTC_DATA_CHANNEL_LABEL,
            WebRtcSignal, WebRtcTerminalReason,
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
    endpoint: Endpoint,
    webrtc_endpoint: StdMutex<Endpoint>,
    relay_router: StdMutex<Option<Router>>,
    webrtc_router: StdMutex<Option<Router>>,
    transport: WebRtcTransport,
    config: WebRtcNodeConfig,
    state: StdMutex<NativeNodeState>,
}

#[derive(Debug)]
struct NativeNodeState {
    closed: bool,
    dial_ids: DialIdGenerator,
    accept_queues: HashMap<Vec<u8>, NativeAcceptQueue>,
    protocol_alpns: Vec<Vec<u8>>,
    sessions: Vec<NativeWebRtcSession>,
    session_routes: Vec<NativeSessionRoute>,
}

#[derive(Debug, Clone)]
struct NativeSessionRoute {
    remote: EndpointId,
    dial_id: DialId,
    alpn: String,
}

#[derive(Debug)]
struct NativeAcceptQueue {
    tx: mpsc::Sender<Connection>,
    rx: Arc<AsyncMutex<mpsc::Receiver<Connection>>>,
}

/// Builder for a native WebRTC Iroh facade node.
pub struct NativeWebRtcIrohNodeBuilder {
    config: WebRtcNodeConfig,
    secret_key: SecretKey,
    endpoint_builder: Builder,
    facade_alpns: Vec<Vec<u8>>,
    protocols: Vec<NativeProtocolRegistration>,
}

struct NativeProtocolRegistration {
    alpn: Vec<u8>,
    relay: Box<dyn Fn() -> Box<dyn DynProtocolHandler> + Send + Sync>,
    webrtc: Box<dyn Fn() -> Box<dyn DynProtocolHandler> + Send + Sync>,
}

impl NativeWebRtcIrohNode {
    /// Creates a builder for a native facade node.
    pub fn builder(config: WebRtcNodeConfig, secret_key: SecretKey) -> NativeWebRtcIrohNodeBuilder {
        NativeWebRtcIrohNodeBuilder {
            config,
            secret_key,
            endpoint_builder: Endpoint::builder(presets::N0),
            facade_alpns: Vec::new(),
            protocols: Vec::new(),
        }
    }

    /// Binds a native facade node with the default Iroh endpoint preset.
    pub async fn bind(config: WebRtcNodeConfig, secret_key: SecretKey) -> Result<Self> {
        Self::builder(config, secret_key).spawn().await
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
        Self::builder(config, secret_key)
            .endpoint_builder(builder)
            .spawn()
            .await
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
        let (relay_router, webrtc_router, sessions) = {
            let mut state = self
                .inner
                .state
                .lock()
                .expect("native facade node mutex poisoned");
            if state.closed {
                return;
            }
            state.closed = true;
            state.accept_queues.clear();
            let sessions = std::mem::take(&mut state.sessions);
            state.session_routes.clear();
            let relay_router = self
                .inner
                .relay_router
                .lock()
                .expect("native facade relay router mutex poisoned")
                .take();
            let webrtc_router = self
                .inner
                .webrtc_router
                .lock()
                .expect("native facade WebRTC router mutex poisoned")
                .take();
            (relay_router, webrtc_router, sessions)
        };
        if let Some(router) = relay_router {
            let _ = router.shutdown().await;
        }
        if let Some(router) = webrtc_router {
            let _ = router.shutdown().await;
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
        self.track_session_route(NativeSessionRoute {
            remote: remote_id,
            dial_id: coordinator.dial_id(),
            alpn: alpn_string.clone(),
        })?;

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
        let webrtc_endpoint = self.webrtc_endpoint();
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

    fn track_session_route(&self, route: NativeSessionRoute) -> Result<()> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("native facade node mutex poisoned");
        if state.closed {
            bail!("native WebRTC node is closed");
        }
        state.session_routes.push(route);
        Ok(())
    }

    fn accept_receiver(
        &self,
        alpn: Vec<u8>,
    ) -> Result<Arc<AsyncMutex<mpsc::Receiver<Connection>>>> {
        let state = self
            .inner
            .state
            .lock()
            .expect("native facade node mutex poisoned");
        if state.closed {
            bail!("native WebRTC node is closed");
        }
        let receiver = state
            .accept_queues
            .get(&alpn)
            .with_context(|| "facade ALPN was not registered before native WebRTC node spawn")?
            .rx
            .clone();
        Ok(receiver)
    }

    fn webrtc_endpoint(&self) -> Endpoint {
        self.inner
            .webrtc_endpoint
            .lock()
            .expect("native facade WebRTC endpoint mutex poisoned")
            .clone()
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
        self.track_session_route(NativeSessionRoute {
            remote: from,
            dial_id: coordinator.dial_id(),
            alpn,
        })?;

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
        } else if state
            .protocol_alpns
            .iter()
            .any(|registered| registered.as_slice() == alpn)
        {
            Ok(())
        } else {
            bail!("no active native WebRTC accept registration for ALPN")
        }
    }

    fn matching_acceptor_session(&self, remote: EndpointId, alpn: &str) -> Option<WebRtcAddr> {
        let state = self
            .inner
            .state
            .lock()
            .expect("native facade node mutex poisoned");
        state
            .session_routes
            .iter()
            .find(|route| route.remote == remote && route.alpn == alpn)
            .map(|route| WebRtcAddr::session(remote, route.dial_id.0))
    }
}

impl NativeWebRtcIrohNodeBuilder {
    /// Uses a caller-provided Iroh endpoint builder for the relay/bootstrap endpoint.
    pub fn endpoint_builder(mut self, builder: Builder) -> Self {
        self.endpoint_builder = builder;
        self
    }

    /// Registers a full Iroh protocol handler on both relay and WebRTC endpoints.
    pub fn accept_protocol<H>(mut self, alpn: impl AsRef<[u8]>, handler: H) -> Self
    where
        H: ProtocolHandler + Clone,
    {
        let alpn = alpn.as_ref().to_vec();
        let relay_handler = handler.clone();
        let webrtc_handler = handler;
        self.protocols.push(NativeProtocolRegistration {
            alpn,
            relay: Box::new(move || Box::new(relay_handler.clone())),
            webrtc: Box::new(move || Box::new(webrtc_handler.clone())),
        });
        self
    }

    /// Registers a simple stream facade accepted by `NativeWebRtcIrohNode::accept`.
    pub fn accept_facade(mut self, alpn: impl AsRef<[u8]>) -> Self {
        self.facade_alpns.push(alpn.as_ref().to_vec());
        self
    }

    /// Binds endpoints, installs Iroh routers, and starts accepting registered protocols.
    pub async fn spawn(self) -> Result<NativeWebRtcIrohNode> {
        self.config
            .validate()
            .context("invalid native WebRTC node config")?;
        let transport = WebRtcTransport::try_new(self.config.transport.clone())
            .context("invalid native WebRTC transport config")?;
        let endpoint = self
            .endpoint_builder
            .secret_key(self.secret_key.clone())
            .bind()
            .await
            .context("failed to bind native WebRTC bootstrap endpoint")?;
        let webrtc_endpoint =
            bind_webrtc_application_endpoint(self.secret_key.clone(), transport.clone())
                .await
                .context("failed to bind native WebRTC application endpoint")?;
        let endpoint_id = endpoint.id();

        let mut accept_queues = HashMap::new();
        for alpn in self.facade_alpns {
            let (tx, rx) = mpsc::channel(16);
            accept_queues.insert(
                alpn,
                NativeAcceptQueue {
                    tx,
                    rx: Arc::new(AsyncMutex::new(rx)),
                },
            );
        }
        let protocol_alpns = self
            .protocols
            .iter()
            .map(|registration| registration.alpn.clone())
            .collect::<Vec<_>>();

        let node = NativeWebRtcIrohNode {
            inner: Arc::new(NativeNodeInner {
                endpoint: endpoint.clone(),
                webrtc_endpoint: StdMutex::new(webrtc_endpoint.clone()),
                relay_router: StdMutex::new(None),
                webrtc_router: StdMutex::new(None),
                transport,
                config: self.config,
                state: StdMutex::new(NativeNodeState {
                    closed: false,
                    dial_ids: DialIdGenerator::new(endpoint_id),
                    accept_queues,
                    protocol_alpns,
                    sessions: Vec::new(),
                    session_routes: Vec::new(),
                }),
            }),
        };

        let mut relay_router = Router::builder(endpoint.clone()).accept(
            WEBRTC_BOOTSTRAP_ALPN,
            NativeBootstrapHandler { node: node.clone() },
        );
        let mut webrtc_router = Router::builder(webrtc_endpoint.clone());
        let accept_relay_app_connections = node
            .inner
            .config
            .default_dial_options
            .transport_intent
            .allows_iroh_relay_fallback()
            || !node
                .inner
                .config
                .default_dial_options
                .transport_intent
                .uses_webrtc();
        {
            let state = node
                .inner
                .state
                .lock()
                .expect("native facade node mutex poisoned");
            for (alpn, queue) in &state.accept_queues {
                let handler = FacadeAcceptHandler {
                    tx: queue.tx.clone(),
                };
                if accept_relay_app_connections {
                    relay_router = relay_router.accept(alpn, handler.clone());
                }
                webrtc_router = webrtc_router.accept(
                    alpn,
                    WebRtcSessionGate::new(node.clone(), Box::new(handler)),
                );
            }
        }
        for registration in self.protocols {
            if accept_relay_app_connections {
                relay_router = relay_router.accept(&registration.alpn, (registration.relay)());
            }
            webrtc_router = webrtc_router.accept(
                &registration.alpn,
                WebRtcSessionGate::new(node.clone(), (registration.webrtc)()),
            );
        }

        *node
            .inner
            .relay_router
            .lock()
            .expect("native facade relay router mutex poisoned") = Some(relay_router.spawn());
        *node
            .inner
            .webrtc_router
            .lock()
            .expect("native facade WebRTC router mutex poisoned") = Some(webrtc_router.spawn());
        Ok(node)
    }
}

#[derive(Debug, Clone)]
struct NativeBootstrapHandler {
    node: NativeWebRtcIrohNode,
}

impl ProtocolHandler for NativeBootstrapHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        self.node
            .handle_bootstrap_connection(connection)
            .await
            .map_err(|err| AcceptError::from_boxed(err.into()))
    }
}

#[derive(Debug, Clone)]
struct FacadeAcceptHandler {
    tx: mpsc::Sender<Connection>,
}

impl ProtocolHandler for FacadeAcceptHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        if self.tx.send(connection.clone()).await.is_ok() {
            Ok(())
        } else {
            connection.close(0u32.into(), b"native facade accept queue closed");
            Err(AcceptError::from_err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "native facade accept queue closed",
            )))
        }
    }
}

#[derive(Debug)]
struct WebRtcSessionGate {
    node: NativeWebRtcIrohNode,
    handler: Box<dyn DynProtocolHandler>,
}

impl WebRtcSessionGate {
    fn new(node: NativeWebRtcIrohNode, handler: Box<dyn DynProtocolHandler>) -> Self {
        Self { node, handler }
    }
}

impl ProtocolHandler for WebRtcSessionGate {
    async fn on_accepting(
        &self,
        accepting: Accepting,
    ) -> std::result::Result<Connection, AcceptError> {
        let connection = self.handler.on_accepting(accepting).await?;
        let alpn = String::from_utf8_lossy(connection.alpn()).to_string();
        let expected = match self
            .node
            .matching_acceptor_session(connection.remote_id(), &alpn)
        {
            Some(expected) => expected,
            None => {
                connection.close(
                    0u32.into(),
                    b"custom app connection did not match WebRTC session",
                );
                return Err(AcceptError::from_err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "custom app connection did not match WebRTC session",
                )));
            }
        };
        if let Err(err) = require_webrtc_selected_path(&connection, expected) {
            connection.close(0u32.into(), b"WebRTC custom path was not selected");
            return Err(AcceptError::from_boxed(err.into()));
        }
        Ok(connection)
    }

    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        self.handler.accept(connection).await
    }

    async fn shutdown(&self) {
        self.handler.shutdown().await;
    }
}

async fn bind_webrtc_application_endpoint(
    secret_key: SecretKey,
    transport: WebRtcTransport,
) -> Result<Endpoint> {
    let builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret_key)
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
