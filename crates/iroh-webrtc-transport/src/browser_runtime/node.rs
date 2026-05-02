use super::*;
use std::{sync::atomic::Ordering, time::Duration};

const BROWSER_STREAM_IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(in crate::browser_runtime) struct BrowserRuntimeNode {
    inner: Arc<Mutex<BrowserRuntimeNodeInner>>,
}

impl std::fmt::Debug for BrowserRuntimeNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserRuntimeNode").finish_non_exhaustive()
    }
}

struct BrowserRuntimeNodeInner {
    secret_key: SecretKey,
    endpoint_id: EndpointId,
    local_custom_addr: CustomAddr,
    transport: WebRtcTransport,
    relay_endpoint: Option<Endpoint>,
    relay_router: Option<Router>,
    webrtc_endpoint: Option<Endpoint>,
    webrtc_router: Option<Router>,
    benchmark_echo_tasks: HashMap<String, n0_future::task::AbortHandle>,
    bootstrap_connection_tx: Option<mpsc::UnboundedSender<Connection>>,
    browser_protocols: BrowserProtocolRegistry,
    benchmark_echo_alpns: Vec<String>,
    protocol_transport_intent: BootstrapTransportIntent,
    protocol_transport_lookup: MemoryLookup,
    protocol_transport_prepare_tx: Option<mpsc::Sender<ProtocolTransportPrepareRequest>>,
    dial_ids: DialIdGenerator,
    session_config: WebRtcSessionConfig,
    low_latency_quic_acks: bool,
    sessions: HashMap<BrowserSessionKey, BrowserSessionState>,
    connections: HashMap<BrowserConnectionKey, BrowserConnectionState>,
    streams: HashMap<BrowserStreamKey, Arc<BrowserStreamState>>,
    accepts: HashMap<String, AcceptRegistrationState>,
    next_connection_key: u64,
    next_stream_key: u64,
    closed: bool,
}

mod accept_state;
mod benchmark;
mod connection;
mod connection_state;
mod core;
mod protocol_transport;
mod rtc;
mod session;
mod session_state;
mod stream;
mod stream_state;

use accept_state::AcceptRegistrationState;
use connection_state::BrowserConnectionState;
use session_state::BrowserSessionState;
use stream_state::{BrowserStreamState, notify_stream_cancel};

impl BrowserRuntimeNode {
    fn update_session(
        &self,
        session_key: &BrowserSessionKey,
        update: impl FnOnce(&mut BrowserSessionState) -> BrowserRuntimeResult<()>,
    ) -> BrowserRuntimeResult<BrowserSessionSnapshot> {
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
        update(session)?;
        let snapshot = session.snapshot();
        inner.refresh_webrtc_transport_addrs();
        Ok(snapshot)
    }
}

impl BrowserRuntimeNodeInner {
    fn refresh_webrtc_transport_addrs(&self) {
        let mut addrs = vec![WebRtcAddr::capability(self.endpoint_id).to_custom_addr()];
        addrs.extend(self.sessions.values().filter_map(|session| {
            if !session.transport_intent.uses_webrtc()
                || matches!(
                    session.channel_attachment,
                    DataChannelAttachmentState::Closed | DataChannelAttachmentState::Failed
                )
                || matches!(
                    session.lifecycle,
                    BrowserSessionLifecycle::Closed | BrowserSessionLifecycle::Failed
                )
            {
                return None;
            }
            Some(WebRtcAddr::session(session.local, session.dial_id.0).to_custom_addr())
        }));
        addrs.sort_by(|left, right| left.data().cmp(right.data()));
        addrs.dedup();
        self.transport.set_local_addrs(addrs);
    }

    fn allocate_connection_key(&mut self) -> BrowserConnectionKey {
        let key = BrowserConnectionKey(self.next_connection_key);
        self.next_connection_key = self
            .next_connection_key
            .checked_add(1)
            .expect("browser connection id exhausted");
        key
    }

    fn accept_registration_mut(
        &mut self,
        accept_id: BrowserAcceptId,
    ) -> BrowserRuntimeResult<&mut AcceptRegistrationState> {
        self.accepts
            .values_mut()
            .find(|registration| registration.id == accept_id)
            .ok_or_else(|| {
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::UnsupportedAlpn,
                    "unknown accept registration",
                )
            })
    }

    fn accept_alpn_for_id(&self, accept_id: BrowserAcceptId) -> Option<String> {
        self.accepts
            .values()
            .find(|registration| registration.id == accept_id)
            .map(|registration| registration.alpn.clone())
    }
}

fn validate_transport_resolution(
    session: &BrowserSessionState,
    resolved_transport: BrowserResolvedTransport,
) -> BrowserRuntimeResult<()> {
    if resolved_transport == BrowserResolvedTransport::WebRtc
        && session.transport_intent == BootstrapTransportIntent::IrohRelay
    {
        return Err(BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::WebRtcFailed,
            "relay-only session cannot resolve as WebRTC",
        ));
    }
    if resolved_transport == BrowserResolvedTransport::IrohRelay
        && session.transport_intent == BootstrapTransportIntent::WebRtcOnly
    {
        return Err(BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::WebRtcFailed,
            "WebRTC-only session cannot resolve through Iroh relay",
        ));
    }
    if resolved_transport == BrowserResolvedTransport::WebRtc
        && session.channel_attachment != DataChannelAttachmentState::Open
    {
        return Err(BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::DataChannelFailed,
            "WebRTC session cannot be promoted before transferred DataChannel opens",
        ));
    }
    Ok(())
}

fn session_terminal_signal(
    session: &BrowserSessionState,
    reason: WebRtcTerminalReason,
    message: Option<String>,
) -> WebRtcSignal {
    let coordinator = DialCoordinator::from_parts_with_transport_intent(
        session.local,
        session.remote,
        session.generation,
        session.dial_id,
        session.transport_intent,
    );
    match message {
        Some(message) => coordinator.terminal_with_message(reason, message),
        None => coordinator.terminal(reason),
    }
}

fn trace_iroh_connection_paths(
    label: &'static str,
    connection_key: Option<u64>,
    connection: &Connection,
) {
    let paths = connection.paths().into_iter().collect::<Vec<_>>();
    let path_count = paths.len();
    let selected_paths = paths
        .iter()
        .filter(|path| path.is_selected())
        .map(|path| describe_iroh_path(connection, path))
        .collect::<Vec<_>>()
        .join(" | ");
    let all_paths = paths
        .iter()
        .map(|path| describe_iroh_path(connection, path))
        .collect::<Vec<_>>()
        .join(" | ");
    if should_debug_path_snapshot(label) {
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_runtime::connection",
            label,
            connection_key,
            remote = %connection.remote_id(),
            alpn = %String::from_utf8_lossy(connection.alpn()),
            side = ?connection.side(),
            path_count,
            selected_paths = %selected_paths,
            all_paths = %all_paths,
            "Iroh connection path snapshot"
        );
    } else {
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::connection",
            label,
            connection_key,
            remote = %connection.remote_id(),
            alpn = %String::from_utf8_lossy(connection.alpn()),
            side = ?connection.side(),
            path_count,
            selected_paths = %selected_paths,
            all_paths = %all_paths,
            "Iroh connection path snapshot"
        );
    }
}

fn should_debug_path_snapshot(label: &str) -> bool {
    matches!(
        label,
        "benchmark throughput after warmup"
            | "benchmark throughput after upload"
            | "benchmark throughput after download"
            | "benchmark throughput complete"
            | "benchmark echo after upload receive"
            | "benchmark echo after download send"
    )
}

fn describe_iroh_path(connection: &Connection, path: &iroh::endpoint::PathInfo) -> String {
    let kind = if path.is_relay() {
        "relay"
    } else if path.is_ip() {
        "ip"
    } else {
        "custom"
    };
    let stats = path.stats();
    let rtt_ms = stats.map(|stats| stats.rtt.as_millis());
    let path_cwnd = stats.map(|stats| stats.cwnd);
    let current_mtu = stats.map(|stats| stats.current_mtu);
    let lost_packets = stats.map(|stats| stats.lost_packets);
    let lost_bytes = stats.map(|stats| stats.lost_bytes);
    let tx_datagrams = stats.map(|stats| stats.udp_tx.datagrams);
    let tx_bytes = stats.map(|stats| stats.udp_tx.bytes);
    let rx_datagrams = stats.map(|stats| stats.udp_rx.datagrams);
    let rx_bytes = stats.map(|stats| stats.udp_rx.bytes);
    let congestion = connection
        .congestion_state(path.id())
        .map(|controller| controller.metrics());
    let congestion_window = congestion.as_ref().map(|metrics| metrics.congestion_window);
    let ssthresh = congestion.as_ref().and_then(|metrics| metrics.ssthresh);
    let pacing_bps = congestion.as_ref().and_then(|metrics| metrics.pacing_rate);
    format!(
        "id={} kind={} selected={} closed={} remote={:?} rtt_ms={:?} cwnd={:?} controller_cwnd={:?} ssthresh={:?} pacing_bps={:?} mtu={:?} lost_packets={:?} lost_bytes={:?} tx_datagrams={:?} tx_bytes={:?} rx_datagrams={:?} rx_bytes={:?}",
        path.id(),
        kind,
        path.is_selected(),
        path.is_closed(),
        path.remote_addr(),
        rtt_ms,
        path_cwnd,
        congestion_window,
        ssthresh,
        pacing_bps,
        current_mtu,
        lost_packets,
        lost_bytes,
        tx_datagrams,
        tx_bytes,
        rx_datagrams,
        rx_bytes
    )
}

fn require_webrtc_selected_path(
    connection: &Connection,
    expected_remote: EndpointId,
    expected_dial_id: DialId,
) -> BrowserRuntimeResult<()> {
    let expected = WebRtcAddr::session(expected_remote, expected_dial_id.0);

    for path in connection.paths().into_iter() {
        if !path.is_selected() {
            continue;
        }
        let TransportAddr::Custom(custom_addr) = path.remote_addr() else {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                format!(
                    "WebRTC application connection selected non-custom Iroh path: {:?}",
                    path.remote_addr()
                ),
            ));
        };
        let actual = WebRtcAddr::from_custom_addr(custom_addr).map_err(|err| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                format!("selected custom path is not a WebRTC session address: {err}"),
            )
        })?;
        if actual == expected {
            return Ok(());
        }
        return Err(BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::WebRtcFailed,
            format!(
                "selected WebRTC custom path does not match session: expected {:?}, got {:?}",
                expected, actual
            ),
        ));
    }

    Err(BrowserRuntimeError::new(
        BrowserRuntimeErrorCode::WebRtcFailed,
        "WebRTC application connection has no selected Iroh path",
    ))
}

fn connection_info(connection: &BrowserAcceptedConnection) -> BrowserConnectionInfo {
    BrowserConnectionInfo {
        connection_key: connection_key_string(connection.key),
        remote_endpoint: endpoint_id_string(connection.remote),
        alpn: connection.alpn.clone(),
        transport: connection.transport,
        dial_id: connection
            .session_key
            .as_ref()
            .map(|session_key| session_key.as_str().to_owned()),
        session_key: connection
            .session_key
            .as_ref()
            .map(|session_key| session_key.as_str().to_owned()),
    }
}
