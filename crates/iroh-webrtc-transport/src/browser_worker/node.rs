use super::*;
use std::{sync::atomic::Ordering, time::Duration};

const WORKER_STREAM_IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(in crate::browser_worker) struct BrowserWorkerNode {
    inner: Arc<Mutex<BrowserWorkerNodeInner>>,
}

struct BrowserWorkerNodeInner {
    secret_key: SecretKey,
    node_key: WorkerNodeKey,
    endpoint_id: EndpointId,
    local_custom_addr: CustomAddr,
    transport: WebRtcTransport,
    relay_endpoint: Option<Endpoint>,
    relay_accept_abort: Option<n0_future::task::AbortHandle>,
    webrtc_endpoint: Option<Endpoint>,
    webrtc_accept_abort: Option<n0_future::task::AbortHandle>,
    benchmark_echo_tasks: HashMap<String, n0_future::task::AbortHandle>,
    bootstrap_connection_tx: Option<mpsc::UnboundedSender<Connection>>,
    dial_ids: DialIdGenerator,
    accept_queue_capacity: usize,
    session_config: WebRtcSessionConfig,
    low_latency_quic_acks: bool,
    sessions: HashMap<WorkerSessionKey, WorkerSessionState>,
    connections: HashMap<WorkerConnectionKey, WorkerConnectionState>,
    streams: HashMap<WorkerStreamKey, Arc<WorkerStreamState>>,
    accepts: HashMap<String, AcceptRegistrationState>,
    next_accept_id: u64,
    next_connection_key: u64,
    next_stream_key: u64,
    closed: bool,
}

mod accept_state;
mod benchmark;
mod connection;
mod connection_state;
mod core;
mod rtc;
mod session;
mod session_state;
mod stream;
mod stream_state;

use accept_state::AcceptRegistrationState;
use connection_state::WorkerConnectionState;
use session_state::WorkerSessionState;
use stream_state::{WorkerStreamState, notify_stream_cancel};

impl BrowserWorkerNode {
    fn update_session(
        &self,
        session_key: &WorkerSessionKey,
        update: impl FnOnce(&mut WorkerSessionState) -> BrowserWorkerResult<()>,
    ) -> BrowserWorkerResult<WorkerSessionSnapshot> {
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
        update(session)?;
        let snapshot = session.snapshot();
        inner.refresh_webrtc_transport_addrs();
        Ok(snapshot)
    }
}

impl BrowserWorkerNodeInner {
    fn relay_endpoint_alpns(&self) -> Vec<Vec<u8>> {
        let mut alpns = Vec::with_capacity(self.accepts.len() + 1);
        alpns.push(WEBRTC_BOOTSTRAP_ALPN.to_vec());
        alpns.extend(self.accepts.keys().map(|alpn| alpn.as_bytes().to_vec()));
        alpns
    }

    fn webrtc_endpoint_alpns(&self) -> Vec<Vec<u8>> {
        self.accepts
            .keys()
            .map(|alpn| alpn.as_bytes().to_vec())
            .collect()
    }

    fn refresh_endpoint_alpns(&self) {
        if let Some(endpoint) = &self.relay_endpoint {
            endpoint.set_alpns(self.relay_endpoint_alpns());
        }
        if let Some(endpoint) = &self.webrtc_endpoint {
            endpoint.set_alpns(self.webrtc_endpoint_alpns());
        }
    }

    fn refresh_webrtc_transport_addrs(&self) {
        let mut addrs = vec![WebRtcAddr::capability(self.endpoint_id).to_custom_addr()];
        addrs.extend(self.sessions.values().filter_map(|session| {
            if !session.transport_intent.uses_webrtc()
                || matches!(
                    session.channel_attachment,
                    WorkerDataChannelAttachmentState::Closed
                        | WorkerDataChannelAttachmentState::Failed
                )
                || matches!(
                    session.lifecycle,
                    WorkerSessionLifecycle::Closed | WorkerSessionLifecycle::Failed
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

    fn allocate_connection_key(&mut self) -> WorkerConnectionKey {
        let key = WorkerConnectionKey(self.next_connection_key);
        self.next_connection_key = self
            .next_connection_key
            .checked_add(1)
            .expect("worker connection id exhausted");
        key
    }

    fn accept_registration_mut(
        &mut self,
        accept_id: WorkerAcceptId,
    ) -> BrowserWorkerResult<&mut AcceptRegistrationState> {
        self.accepts
            .values_mut()
            .find(|registration| registration.id == accept_id)
            .ok_or_else(|| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::UnsupportedAlpn,
                    "unknown accept registration",
                )
            })
    }

    fn accept_alpn_for_id(&self, accept_id: WorkerAcceptId) -> Option<String> {
        self.accepts
            .values()
            .find(|registration| registration.id == accept_id)
            .map(|registration| registration.alpn.clone())
    }
}

fn validate_transport_resolution(
    session: &WorkerSessionState,
    resolved_transport: WorkerResolvedTransport,
) -> BrowserWorkerResult<()> {
    if resolved_transport == WorkerResolvedTransport::WebRtc
        && session.transport_intent == BootstrapTransportIntent::IrohRelay
    {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "relay-only session cannot resolve as WebRTC",
        ));
    }
    if resolved_transport == WorkerResolvedTransport::IrohRelay
        && session.transport_intent == BootstrapTransportIntent::WebRtcOnly
    {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "WebRTC-only session cannot resolve through Iroh relay",
        ));
    }
    if resolved_transport == WorkerResolvedTransport::WebRtc
        && session.channel_attachment != WorkerDataChannelAttachmentState::Open
    {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::DataChannelFailed,
            "WebRTC session cannot be promoted before transferred DataChannel opens",
        ));
    }
    Ok(())
}

fn session_terminal_signal(
    session: &WorkerSessionState,
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

fn close_peer_connection_commands(decision: &WorkerTerminalDecision) -> Vec<WorkerMainRtcCommand> {
    vec![WorkerMainRtcCommand::ClosePeerConnection(
        WorkerMainClosePeerConnectionPayload {
            session_key: decision.session_key.clone(),
            reason: Some(terminal_reason_string(decision.reason).to_owned()),
        },
    )]
}

fn terminal_reason_string(reason: WebRtcTerminalReason) -> &'static str {
    match reason {
        WebRtcTerminalReason::WebRtcFailed => "webrtcFailed",
        WebRtcTerminalReason::FallbackSelected => "fallbackSelected",
        WebRtcTerminalReason::Cancelled => "cancelled",
        WebRtcTerminalReason::Closed => "closed",
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
            target: "iroh_webrtc_transport::browser_worker::connection",
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
            target: "iroh_webrtc_transport::browser_worker::connection",
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
) -> BrowserWorkerResult<()> {
    let expected = WebRtcAddr::session(expected_remote, expected_dial_id.0);

    for path in connection.paths().into_iter() {
        if !path.is_selected() {
            continue;
        }
        let TransportAddr::Custom(custom_addr) = path.remote_addr() else {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!(
                    "WebRTC application connection selected non-custom Iroh path: {:?}",
                    path.remote_addr()
                ),
            ));
        };
        let actual = WebRtcAddr::from_custom_addr(custom_addr).map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("selected custom path is not a WebRTC session address: {err}"),
            )
        })?;
        if actual == expected {
            return Ok(());
        }
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!(
                "selected WebRTC custom path does not match session: expected {:?}, got {:?}",
                expected, actual
            ),
        ));
    }

    Err(BrowserWorkerError::new(
        BrowserWorkerErrorCode::WebRtcFailed,
        "WebRTC application connection has no selected Iroh path",
    ))
}

fn connection_info(connection: &WorkerAcceptedConnection) -> WorkerProtocolConnectionInfo {
    WorkerProtocolConnectionInfo {
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
