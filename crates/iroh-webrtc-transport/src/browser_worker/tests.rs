use super::*;
use n0_future::{
    task,
    time::{sleep, timeout},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex as StdMutex},
};

fn node_with_accept_capacity(capacity: usize) -> BrowserWorkerNode {
    BrowserWorkerNode::spawn(BrowserWorkerNodeConfig {
        accept_queue_capacity: capacity,
        facade_alpns: vec!["app/alpn".into()],
        ..BrowserWorkerNodeConfig::default()
    })
    .unwrap()
}

fn node_with_facade_alpn(alpn: &str) -> BrowserWorkerNode {
    BrowserWorkerNode::spawn(BrowserWorkerNodeConfig {
        facade_alpns: vec![alpn.into()],
        ..BrowserWorkerNodeConfig::default()
    })
    .unwrap()
}

fn spawn_payload_with_facade() -> Value {
    json!({ "facadeAlpns": ["app/alpn"] })
}

fn node_with_benchmark_alpn(alpn: &str) -> BrowserWorkerNode {
    BrowserWorkerNode::spawn(BrowserWorkerNodeConfig {
        benchmark_echo_alpns: vec![alpn.into()],
        ..BrowserWorkerNodeConfig::default()
    })
    .unwrap()
}

fn endpoint_id() -> EndpointId {
    SecretKey::generate().public()
}

fn assert_error_code<T: std::fmt::Debug>(
    result: BrowserWorkerResult<T>,
    code: BrowserWorkerErrorCode,
) -> BrowserWorkerError {
    let err = result.expect_err("operation should fail");
    assert_eq!(err.code, code);
    err
}

fn assert_wire_error_code(
    result: Result<Value, BrowserWorkerWireError>,
    code: BrowserWorkerErrorCode,
) -> BrowserWorkerWireError {
    let err = result.expect_err("operation should fail");
    assert_eq!(err.code, code);
    err
}

#[test]
fn spawn_payload_config_parses_low_latency_quic_acks() {
    let command = WorkerCommand::decode(
        WORKER_SPAWN_COMMAND,
        json!({
            "lowLatencyQuicAcks": true
        }),
    )
    .unwrap();
    let WorkerCommand::Spawn { config } = command else {
        panic!("spawn payload decoded to wrong command");
    };
    assert!(config.low_latency_quic_acks);
}

#[test]
fn spawn_payload_config_parses_registered_router_alpns() {
    let command = WorkerCommand::decode(
        WORKER_SPAWN_COMMAND,
        json!({
            "facadeAlpns": ["app/alpn"],
            "benchmarkEchoAlpns": ["bench/alpn"]
        }),
    )
    .unwrap();
    let WorkerCommand::Spawn { config } = command else {
        panic!("spawn payload decoded to wrong command");
    };
    assert_eq!(config.facade_alpns, vec!["app/alpn"]);
    assert_eq!(config.benchmark_echo_alpns, vec!["bench/alpn"]);
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
enum TestProtocolCommand {
    Join { topic: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
enum TestProtocolEvent {
    Joined { topic: String },
}

#[derive(Debug, Clone, Default)]
struct TestProtocol {
    commands: Arc<StdMutex<Vec<TestProtocolCommand>>>,
    events: Arc<StdMutex<VecDeque<TestProtocolEvent>>>,
}

impl crate::browser::BrowserWorkerProtocol for TestProtocol {
    const ALPN: &'static [u8] = b"test/browser-worker-protocol/1";

    type Command = TestProtocolCommand;
    type Event = TestProtocolEvent;
    type Handler = TestProtocolHandler;

    fn handler(&self, _endpoint: Endpoint) -> Self::Handler {
        TestProtocolHandler
    }

    async fn handle_command(&self, command: Self::Command) -> crate::Result<()> {
        self.commands
            .lock()
            .expect("test protocol mutex poisoned")
            .push(command);
        Ok(())
    }

    async fn next_event(&self) -> crate::Result<Option<Self::Event>> {
        Ok(self
            .events
            .lock()
            .expect("test protocol mutex poisoned")
            .pop_front())
    }
}

#[derive(Debug, Clone)]
struct TestProtocolHandler;

impl ProtocolHandler for TestProtocolHandler {
    async fn accept(&self, _connection: Connection) -> std::result::Result<(), AcceptError> {
        Ok(())
    }
}

#[tokio::test]
async fn typed_worker_protocol_registry_round_trips_command_and_event() {
    let protocol = TestProtocol::default();
    protocol
        .events
        .lock()
        .expect("test protocol mutex poisoned")
        .push_back(TestProtocolEvent::Joined {
            topic: "rust".into(),
        });
    let mut registry = BrowserWorkerProtocolRegistry::new();
    registry.register(protocol.clone()).unwrap();

    let command = json!({ "Join": { "topic": "rust" } });
    let result = registry
        .handle_command("test/browser-worker-protocol/1", command)
        .await
        .unwrap();
    assert_eq!(result, json!({ "handled": true }));
    assert_eq!(
        protocol
            .commands
            .lock()
            .expect("test protocol mutex poisoned")
            .as_slice(),
        &[TestProtocolCommand::Join {
            topic: "rust".into()
        }]
    );

    let event = registry
        .next_event("test/browser-worker-protocol/1")
        .await
        .unwrap();
    assert_eq!(event, json!({ "Joined": { "topic": "rust" } }));
}

#[tokio::test]
async fn worker_protocol_endpoint_connect_rejects_relay_when_webrtc_only_cannot_prepare() {
    let mut registry = BrowserWorkerProtocolRegistry::new();
    registry.register(TestProtocol::default()).unwrap();
    let node = BrowserWorkerNode::spawn_with_secret_key(
        BrowserWorkerNodeConfig {
            worker_protocol_transport_intent: BootstrapTransportIntent::WebRtcOnly,
            ..BrowserWorkerNodeConfig::default()
        },
        SecretKey::generate(),
        registry,
    )
    .unwrap();
    node.start_endpoint().await.unwrap();

    let endpoint = node.webrtc_endpoint().unwrap();
    let remote = endpoint_id();
    let result = timeout(
        std::time::Duration::from_secs(2),
        endpoint.connect(
            remote,
            <TestProtocol as crate::browser::BrowserWorkerProtocol>::ALPN,
        ),
    )
    .await
    .expect("WebRTC-only worker protocol connect should reject without hanging");

    let error = result.unwrap_err().to_string();
    assert!(
        error.to_ascii_lowercase().contains("rejected locally"),
        "unexpected connect error: {error}"
    );
}

#[tokio::test]
async fn worker_protocol_endpoint_connect_requests_webrtc_prepare_before_resolution() {
    let mut registry = BrowserWorkerProtocolRegistry::new();
    registry.register(TestProtocol::default()).unwrap();
    let (prepare_tx, mut prepare_rx) = mpsc::channel(1);
    let node = BrowserWorkerNode::spawn_with_secret_key(
        BrowserWorkerNodeConfig {
            worker_protocol_transport_intent: BootstrapTransportIntent::WebRtcOnly,
            protocol_transport_prepare_tx: Some(prepare_tx),
            ..BrowserWorkerNodeConfig::default()
        },
        SecretKey::generate(),
        registry,
    )
    .unwrap();
    node.start_endpoint().await.unwrap();

    let endpoint = node.webrtc_endpoint().unwrap();
    let remote = endpoint_id();
    let connect = endpoint.connect(
        remote,
        <TestProtocol as crate::browser::BrowserWorkerProtocol>::ALPN,
    );
    tokio::pin!(connect);

    let request = tokio::select! {
        request = prepare_rx.recv() => request.expect("prepare request should be sent"),
        result = &mut connect => panic!("connect finished before transport preparation: {result:?}"),
    };

    assert_eq!(request.remote, remote);
    assert_eq!(
        request.alpn.as_bytes(),
        <TestProtocol as crate::browser::BrowserWorkerProtocol>::ALPN
    );
    assert_eq!(
        request.transport_intent,
        BootstrapTransportIntent::WebRtcOnly
    );

    let dial_id = DialId([7; 16]);
    node.add_protocol_transport_session_addr(remote, dial_id)
        .unwrap();
    request.response.send(Ok(())).unwrap();
    let prepared_addr = node
        .protocol_transport_endpoint_addr(remote)
        .expect("prepared custom address should be stored in worker protocol lookup");

    assert!(prepared_addr.addrs.iter().any(|addr| {
        let TransportAddr::Custom(custom_addr) = addr else {
            return false;
        };
        matches!(
            WebRtcAddr::from_custom_addr(custom_addr),
            Ok(addr) if addr == WebRtcAddr::session(remote, dial_id.0)
        )
    }));
}

#[test]
fn worker_protocol_registry_rejects_duplicate_protocol_type_and_alpn() {
    let mut registry = BrowserWorkerProtocolRegistry::new();
    registry.register(TestProtocol::default()).unwrap();
    assert!(registry.register(TestProtocol::default()).is_err());
}

fn direct_addrs_for(node: &BrowserWorkerNode) -> Vec<String> {
    let addrs = node.endpoint_addr().unwrap();
    let direct_addrs = addrs
        .addrs
        .iter()
        .filter_map(|addr| match addr {
            TransportAddr::Ip(addr) => Some(addr.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !direct_addrs.is_empty(),
        "worker endpoint should expose direct addresses in native tests"
    );
    direct_addrs
}

#[test]
fn duplicate_accept_registration_is_rejected() {
    let node = node_with_facade_alpn("app/alpn");
    let first = node.accept_open("app/alpn").unwrap();

    assert_eq!(first.alpn, "app/alpn");
    assert_error_code(
        node.accept_open("app/alpn"),
        BrowserWorkerErrorCode::UnsupportedAlpn,
    );
}

#[test]
fn accept_close_releases_registration_for_recreation() {
    let node = node_with_facade_alpn("app/alpn");
    let first = node.accept_open("app/alpn").unwrap();

    assert!(node.is_alpn_registered("app/alpn"));
    assert!(node.accept_close(first.id).unwrap());
    assert!(node.is_alpn_registered("app/alpn"));

    let second = node.accept_open("app/alpn").unwrap();
    assert_eq!(first.id, second.id);
    assert!(node.is_alpn_registered("app/alpn"));
}

#[tokio::test]
async fn accept_queue_overflow_rejects_new_connection() {
    let node = node_with_accept_capacity(1);
    let registration = node.accept_open("app/alpn").unwrap();
    let remote = endpoint_id();

    node.admit_incoming_application_connection(
        remote,
        "app/alpn",
        WorkerResolvedTransport::IrohRelay,
        None,
    )
    .unwrap();
    assert_error_code(
        node.admit_incoming_application_connection(
            remote,
            "app/alpn",
            WorkerResolvedTransport::IrohRelay,
            None,
        ),
        BrowserWorkerErrorCode::WebRtcFailed,
    );

    let next = node.accept_next(registration.id).await.unwrap();
    match next {
        WorkerAcceptNext::Ready(connection) => {
            assert_eq!(connection.alpn, "app/alpn");
            assert_eq!(connection.remote, remote);
        }
        WorkerAcceptNext::Done => panic!("queued connection should be delivered"),
    }
}

#[tokio::test]
async fn node_close_completes_pending_accept_next() {
    let node = node_with_facade_alpn("app/alpn");
    let registration = node.accept_open("app/alpn").unwrap();
    let pending = {
        let node = node.clone();
        task::spawn(async move { node.accept_next(registration.id).await })
    };
    tokio::task::yield_now().await;

    assert!(node.close());

    let next = pending.await.unwrap().unwrap();
    assert_eq!(next, WorkerAcceptNext::Done);
    assert!(!node.is_alpn_registered("app/alpn"));
}

#[test]
fn inactive_alpn_is_rejected_for_incoming_connections() {
    let node = BrowserWorkerNode::spawn_default().unwrap();

    assert_error_code(
        node.admit_incoming_application_connection(
            endpoint_id(),
            "inactive/alpn",
            WorkerResolvedTransport::IrohRelay,
            None,
        ),
        BrowserWorkerErrorCode::UnsupportedAlpn,
    );
}

#[test]
fn bootstrap_dial_request_accepts_registered_benchmark_alpn() {
    let node = node_with_benchmark_alpn("bench/alpn");
    let local = node.endpoint_id();
    let remote = endpoint_id();
    let signal = WebRtcSignal::dial_request_with_alpn(
        DialId([8; 16]),
        remote,
        local,
        1,
        "bench/alpn",
        BootstrapTransportIntent::WebRtcOnly,
    );

    let result = node
        .handle_bootstrap_signal(WorkerBootstrapSignalInput { signal, alpn: None })
        .unwrap();

    assert!(result.accepted);
    assert_eq!(result.session.unwrap().alpn, "bench/alpn");
    assert_eq!(result.main_rtc.len(), 1);
}

#[test]
fn simultaneous_same_remote_alpn_dials_allocate_independent_sessions() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let remote = endpoint_id();

    let first = node
        .allocate_dial(
            remote,
            "app/alpn",
            BootstrapTransportIntent::WebRtcPreferred,
        )
        .unwrap();
    let second = node
        .allocate_dial(
            remote,
            "app/alpn",
            BootstrapTransportIntent::WebRtcPreferred,
        )
        .unwrap();

    assert_eq!(first.remote, remote);
    assert_eq!(second.remote, remote);
    assert_eq!(first.alpn, "app/alpn");
    assert_eq!(second.alpn, "app/alpn");
    assert_eq!(first.generation, 0);
    assert_eq!(second.generation, 1);
    assert_ne!(first.dial_id, second.dial_id);
    assert_ne!(first.session_key, second.session_key);
}

#[test]
fn in_flight_and_future_dials_reject_after_close() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let remote = endpoint_id();
    let allocation = node
        .allocate_dial(remote, "app/alpn", BootstrapTransportIntent::IrohRelay)
        .unwrap();

    assert!(node.close());

    assert_error_code(
        node.allocate_dial(
            remote,
            "app/alpn",
            BootstrapTransportIntent::WebRtcPreferred,
        ),
        BrowserWorkerErrorCode::Closed,
    );
    assert_error_code(
        node.complete_dial(&allocation.session_key, WorkerResolvedTransport::IrohRelay),
        BrowserWorkerErrorCode::Closed,
    );
    let snapshot = node.session_snapshot(&allocation.session_key).unwrap();
    assert_eq!(snapshot.lifecycle, WorkerSessionLifecycle::Closed);
}

#[test]
fn endpoint_id_remains_cached_after_close() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let before_state = node.state();

    assert!(node.close());

    let after_state = node.state();
    assert_eq!(node.endpoint_id(), before_state.endpoint_id);
    assert_eq!(after_state.endpoint_id, before_state.endpoint_id);
    assert_eq!(
        after_state.local_custom_addr,
        before_state.local_custom_addr
    );
    assert_eq!(after_state.bootstrap_alpn, before_state.bootstrap_alpn);
    assert!(after_state.closed);
}

#[test]
fn transferred_channel_state_gates_webrtc_dial_completion() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();

    assert_error_code(
        node.complete_dial(&allocation.session_key, WorkerResolvedTransport::WebRtc),
        BrowserWorkerErrorCode::DataChannelFailed,
    );

    let attached = node
        .attach_transferred_channel(&allocation.session_key)
        .unwrap();
    assert_eq!(
        attached.channel_attachment,
        WorkerDataChannelAttachmentState::Transferred
    );

    let opened = node
        .mark_transferred_channel_open(&allocation.session_key)
        .unwrap();
    assert_eq!(
        opened.channel_attachment,
        WorkerDataChannelAttachmentState::Open
    );

    let complete = node
        .complete_dial(&allocation.session_key, WorkerResolvedTransport::WebRtc)
        .unwrap();
    assert_eq!(
        complete.resolved_transport,
        Some(WorkerResolvedTransport::WebRtc)
    );
    assert_eq!(complete.lifecycle, WorkerSessionLifecycle::ApplicationReady);
}

#[test]
fn dial_completion_enforces_transport_preference() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let remote = endpoint_id();
    let relay_only = node
        .allocate_dial(remote, "app/alpn", BootstrapTransportIntent::IrohRelay)
        .unwrap();
    let webrtc_only = node
        .allocate_dial(remote, "app/alpn", BootstrapTransportIntent::WebRtcOnly)
        .unwrap();

    assert_error_code(
        node.complete_dial(&relay_only.session_key, WorkerResolvedTransport::WebRtc),
        BrowserWorkerErrorCode::WebRtcFailed,
    );
    assert_error_code(
        node.complete_dial(&webrtc_only.session_key, WorkerResolvedTransport::IrohRelay),
        BrowserWorkerErrorCode::WebRtcFailed,
    );
}

#[test]
fn bootstrap_dial_request_consumes_transport_intent() {
    let node = node_with_facade_alpn("app/alpn");
    let local = node.endpoint_id();
    let remote = endpoint_id();
    node.accept_open("app/alpn").unwrap();
    let signal = WebRtcSignal::dial_request_with_alpn(
        DialId([7; 16]),
        remote,
        local,
        3,
        "",
        BootstrapTransportIntent::WebRtcOnly,
    );

    let result = node
        .handle_bootstrap_signal(WorkerBootstrapSignalInput {
            signal,
            alpn: Some("app/alpn".into()),
        })
        .unwrap();

    let session = result.session.unwrap();
    assert_eq!(
        session.transport_intent,
        BootstrapTransportIntent::WebRtcOnly
    );
    assert_eq!(session.role, WorkerSessionRole::Acceptor);
    assert_eq!(result.main_rtc.len(), 1);
    match &result.main_rtc[0] {
        WorkerMainRtcCommand::CreatePeerConnection(payload) => {
            assert_eq!(payload.role, WorkerSessionRole::Acceptor);
            assert_eq!(payload.generation, 3);
        }
        other => panic!("unexpected main RTC command: {other:?}"),
    }
}

#[test]
fn terminal_decisions_follow_transport_intent() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let remote = endpoint_id();
    let preferred = node
        .allocate_dial(
            remote,
            "app/alpn",
            BootstrapTransportIntent::WebRtcPreferred,
        )
        .unwrap();
    let only = node
        .allocate_dial(remote, "app/alpn", BootstrapTransportIntent::WebRtcOnly)
        .unwrap();

    let fallback = node
        .decide_webrtc_failure(&preferred.session_key, "ice failed")
        .unwrap();
    assert_eq!(fallback.reason, WebRtcTerminalReason::FallbackSelected);
    assert_eq!(
        fallback.selected_transport,
        Some(WorkerResolvedTransport::IrohRelay)
    );
    assert_eq!(
        fallback.signal.unwrap().terminal_reason(),
        Some(WebRtcTerminalReason::FallbackSelected)
    );
    let preferred_snapshot = node.session_snapshot(&preferred.session_key).unwrap();
    assert_eq!(
        preferred_snapshot.terminal_sent,
        Some(WebRtcTerminalReason::FallbackSelected)
    );
    assert_eq!(
        preferred_snapshot.resolved_transport,
        Some(WorkerResolvedTransport::IrohRelay)
    );
    assert_eq!(
        preferred_snapshot.channel_attachment,
        WorkerDataChannelAttachmentState::Failed
    );
    assert_eq!(
        preferred_snapshot.lifecycle,
        WorkerSessionLifecycle::RelaySelected
    );

    let failed = node
        .decide_webrtc_failure(&only.session_key, "ice failed")
        .unwrap();
    assert_eq!(failed.reason, WebRtcTerminalReason::WebRtcFailed);
    assert_eq!(failed.selected_transport, None);
    assert_eq!(
        failed.signal.unwrap().terminal_reason(),
        Some(WebRtcTerminalReason::WebRtcFailed)
    );
    let only_snapshot = node.session_snapshot(&only.session_key).unwrap();
    assert_eq!(
        only_snapshot.terminal_sent,
        Some(WebRtcTerminalReason::WebRtcFailed)
    );
    assert_eq!(only_snapshot.resolved_transport, None);
    assert_eq!(
        only_snapshot.channel_attachment,
        WorkerDataChannelAttachmentState::Failed
    );
    assert_eq!(only_snapshot.lifecycle, WorkerSessionLifecycle::Failed);
}

#[test]
fn remote_terminal_fallback_updates_session_as_single_transition() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let remote = endpoint_id();
    let allocation = node
        .allocate_dial(remote, "app/alpn", BootstrapTransportIntent::WebRtcOnly)
        .unwrap();

    let result = node
        .handle_bootstrap_signal(WorkerBootstrapSignalInput {
            signal: WebRtcSignal::terminal(
                allocation.dial_id,
                remote,
                allocation.local,
                allocation.generation,
                WebRtcTerminalReason::FallbackSelected,
            ),
            alpn: Some("app/alpn".into()),
        })
        .unwrap();

    assert_eq!(result.connection, None);
    let snapshot = result.session.unwrap();
    assert_eq!(
        snapshot.terminal_received,
        Some(WebRtcTerminalReason::FallbackSelected)
    );
    assert_eq!(snapshot.resolved_transport, None);
    assert_eq!(
        snapshot.channel_attachment,
        WorkerDataChannelAttachmentState::Failed
    );
    assert_eq!(snapshot.lifecycle, WorkerSessionLifecycle::Failed);
}

#[test]
fn cancel_decision_emits_terminal_cancel() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcPreferred,
        )
        .unwrap();

    let cancelled = node
        .cancel_session(&allocation.session_key, "user cancelled")
        .unwrap();

    assert_eq!(cancelled.reason, WebRtcTerminalReason::Cancelled);
    assert_eq!(
        cancelled.signal.unwrap().terminal_reason(),
        Some(WebRtcTerminalReason::Cancelled)
    );
    assert_eq!(
        node.session_snapshot(&allocation.session_key)
            .unwrap()
            .lifecycle,
        WorkerSessionLifecycle::Closed
    );
}

#[test]
fn rtc_control_request_envelope_uses_canonical_main_request_shape() {
    let command = WorkerMainRtcCommand::CreateOffer(WorkerMainSessionPayload {
        session_key: "session-1".into(),
    });
    let (envelope, pending) = main_rtc_command_request_envelope(42, &command).unwrap();

    assert_eq!(envelope["kind"], "request");
    assert_eq!(envelope["id"], 42);
    assert_eq!(envelope["command"], MAIN_RTC_CREATE_OFFER_COMMAND);
    assert_eq!(envelope["payload"]["sessionKey"], "session-1");
    assert_eq!(pending.id, 42);
    assert_eq!(pending.command, MAIN_RTC_CREATE_OFFER_COMMAND);
    assert_eq!(pending.session_key, "session-1");
}

#[test]
fn rtc_control_result_payload_matches_worker_main_rtc_result_shape() {
    let pending = PendingMainRtcRequest {
        id: 7,
        command: MAIN_RTC_CREATE_ANSWER_COMMAND.into(),
        session_key: "session-2".into(),
        generation: None,
        reason: None,
    };

    let payload = worker_main_rtc_result_payload_from_response(
        &pending,
        Some(json!({ "sdp": "answer-sdp" })),
        None,
    )
    .unwrap();

    assert_eq!(payload["sessionKey"], "session-2");
    assert_eq!(payload["command"], MAIN_RTC_CREATE_ANSWER_COMMAND);
    assert_eq!(payload["result"]["sdp"], "answer-sdp");
}

#[test]
fn rtc_control_error_payload_matches_worker_main_rtc_result_shape() {
    let pending = PendingMainRtcRequest {
        id: 7,
        command: MAIN_RTC_CREATE_OFFER_COMMAND.into(),
        session_key: "session-2".into(),
        generation: None,
        reason: None,
    };

    let payload = worker_main_rtc_result_payload_from_response(
        &pending,
        None,
        Some(BrowserWorkerWireError {
            code: BrowserWorkerErrorCode::WebRtcFailed,
            message: "main-thread failure".into(),
        }),
    )
    .unwrap();
    let input = main_rtc_result_input_from_payload(&payload).unwrap();

    assert_eq!(payload["sessionKey"], "session-2");
    assert_eq!(payload["command"], MAIN_RTC_CREATE_OFFER_COMMAND);
    assert_eq!(payload["error"]["code"], "webrtcFailed");
    assert_eq!(payload["error"]["message"], "main-thread failure");
    assert_eq!(
        input.session_key,
        WorkerSessionKey::new("session-2").unwrap()
    );
    assert!(matches!(
        input.event,
        WorkerMainRtcResultEvent::WebRtcFailed { message } if message == "main-thread failure"
    ));
}

#[test]
fn rtc_control_follow_up_commands_decode_from_worker_result() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();
    let session_key = allocation.session_key.clone();
    let result = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: session_key.clone(),
            event: WorkerMainRtcResultEvent::PeerConnectionCreated,
            error_message: None,
        })
        .unwrap();
    let encoded = to_protocol_value(result).unwrap();

    let commands = main_rtc_commands_from_protocol_value(&encoded).unwrap();
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        &commands[0],
        WorkerMainRtcCommand::CreateDataChannel(_)
    ));

    let result = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key,
            event: WorkerMainRtcResultEvent::DataChannelCreated,
            error_message: None,
        })
        .unwrap();
    let mut encoded = to_protocol_value(result).unwrap();
    let commands = main_rtc_commands_from_protocol_value(&encoded).unwrap();
    assert_eq!(commands.len(), 1);
    assert!(matches!(&commands[0], WorkerMainRtcCommand::CreateOffer(_)));

    clear_main_rtc_commands(&mut encoded);
    assert!(encoded.get("mainRtc").is_none());
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
#[test]
fn dispatching_dial_start_without_rtc_control_port_fails() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let start = node
        .allocate_dial_start(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();
    let mut encoded = to_protocol_value(start).unwrap();

    assert!(
        !main_rtc_commands_from_protocol_value(&encoded)
            .unwrap()
            .is_empty()
    );
    let error =
        super::wasm_bindings::dispatch_main_rtc_commands_without_control_for_test(&mut encoded)
            .expect_err("dispatch should fail without an RTC control port");

    assert_eq!(
        error.as_string().as_deref(),
        Some("main-thread RTC control port is not attached")
    );
    assert!(encoded.get("mainRtc").is_some());
}

#[test]
fn main_rtc_result_emits_offer_and_local_ice_protocol_signals() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();
    let session_key = allocation.session_key.clone();

    let peer_created = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: session_key.clone(),
            event: WorkerMainRtcResultEvent::PeerConnectionCreated,
            error_message: None,
        })
        .unwrap();
    assert_eq!(peer_created.main_rtc.len(), 1);
    assert!(matches!(
        &peer_created.main_rtc[0],
        WorkerMainRtcCommand::CreateDataChannel(_)
    ));

    let data_channel_created = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: session_key.clone(),
            event: WorkerMainRtcResultEvent::DataChannelCreated,
            error_message: None,
        })
        .unwrap();
    assert!(matches!(
        &data_channel_created.main_rtc[0],
        WorkerMainRtcCommand::CreateOffer(_)
    ));

    let offer = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: session_key.clone(),
            event: WorkerMainRtcResultEvent::OfferCreated {
                sdp: "offer-sdp".into(),
            },
            error_message: None,
        })
        .unwrap();
    let offer_value = to_protocol_value(offer).unwrap();
    assert_eq!(offer_value["outboundSignals"][0]["type"], "offer");
    assert_eq!(
        offer_value["outboundSignals"][0]["sessionKey"],
        session_key.as_str()
    );
    assert_eq!(offer_value["outboundSignals"][0]["sdp"], "offer-sdp");
    assert_eq!(
        offer_value["mainRtc"][0]["command"],
        MAIN_RTC_NEXT_LOCAL_ICE_COMMAND
    );

    let ice = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: session_key.clone(),
            event: WorkerMainRtcResultEvent::LocalIce {
                ice: WorkerProtocolIceCandidate::Candidate {
                    candidate: "candidate:1".into(),
                    sdp_mid: Some("0".into()),
                    sdp_mline_index: Some(0),
                },
            },
            error_message: None,
        })
        .unwrap();
    let ice_value = to_protocol_value(ice).unwrap();
    assert_eq!(ice_value["outboundSignals"][0]["type"], "ice");
    assert_eq!(ice_value["outboundSignals"][0]["ice"]["type"], "candidate");
    assert_eq!(
        ice_value["outboundSignals"][0]["ice"]["candidate"],
        "candidate:1"
    );
    assert_eq!(
        ice_value["mainRtc"][0]["command"],
        MAIN_RTC_NEXT_LOCAL_ICE_COMMAND
    );
}

#[test]
fn remote_bootstrap_signals_drive_main_rtc_command_sequence() {
    let node = node_with_facade_alpn("app/alpn");
    let local = node.endpoint_id();
    let remote = endpoint_id();
    node.accept_open("app/alpn").unwrap();
    let dial_id = DialId([9; 16]);
    node.handle_bootstrap_signal(WorkerBootstrapSignalInput {
        signal: WebRtcSignal::dial_request_with_alpn(
            dial_id,
            remote,
            local,
            1,
            "",
            BootstrapTransportIntent::WebRtcOnly,
        ),
        alpn: Some("app/alpn".into()),
    })
    .unwrap();

    let offer = node
        .handle_bootstrap_signal(WorkerBootstrapSignalInput {
            signal: WebRtcSignal::Offer {
                dial_id,
                from: remote,
                to: local,
                generation: 1,
                sdp: "offer-sdp".into(),
            },
            alpn: None,
        })
        .unwrap();
    let offer_value = to_protocol_value(offer).unwrap();
    assert_eq!(
        offer_value["mainRtc"][0]["command"],
        MAIN_RTC_ACCEPT_OFFER_COMMAND
    );
    assert!(offer_value["mainRtc"].as_array().unwrap().get(1).is_none());

    let offer_accepted = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: WorkerSessionKey::from(dial_id),
            event: WorkerMainRtcResultEvent::OfferAccepted,
            error_message: None,
        })
        .unwrap();
    let offer_accepted_value = to_protocol_value(offer_accepted).unwrap();
    assert_eq!(
        offer_accepted_value["mainRtc"][0]["command"],
        MAIN_RTC_TAKE_DATA_CHANNEL_COMMAND
    );
    assert_eq!(
        offer_accepted_value["mainRtc"][1]["command"],
        MAIN_RTC_CREATE_ANSWER_COMMAND
    );

    let answer = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: WorkerSessionKey::from(dial_id),
            event: WorkerMainRtcResultEvent::AnswerCreated {
                sdp: "answer-sdp".into(),
            },
            error_message: None,
        })
        .unwrap();
    let answer_value = to_protocol_value(answer).unwrap();
    assert_eq!(answer_value["outboundSignals"][0]["type"], "answer");
    assert_eq!(answer_value["outboundSignals"][0]["sdp"], "answer-sdp");
}

#[test]
fn close_peer_connection_response_is_ack_not_session_close() {
    let result = json!({});
    let event =
        main_rtc_event_from_command(MAIN_RTC_CLOSE_PEER_CONNECTION_COMMAND, Some(&result)).unwrap();

    assert_eq!(
        event,
        WorkerMainRtcResultEvent::PeerConnectionCloseAcknowledged
    );
}

#[test]
fn close_peer_connection_ack_does_not_emit_another_close_command() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();

    let result = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: allocation.session_key.clone(),
            event: WorkerMainRtcResultEvent::PeerConnectionCloseAcknowledged,
            error_message: None,
        })
        .unwrap();

    assert!(result.main_rtc.is_empty());
    assert!(result.outbound_signals.is_empty());
    assert_eq!(
        result.session.unwrap().lifecycle,
        WorkerSessionLifecycle::WebRtcNegotiating
    );
}

#[test]
fn webrtc_dialer_promotes_only_after_transferred_channel_opens() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();

    assert_error_code(
        node.handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: allocation.session_key.clone(),
            event: WorkerMainRtcResultEvent::DataChannelOpen,
            error_message: None,
        }),
        BrowserWorkerErrorCode::DataChannelFailed,
    );

    node.attach_data_channel(&allocation.session_key, WorkerDataChannelSource::Created)
        .unwrap();
    let result = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: allocation.session_key.clone(),
            event: WorkerMainRtcResultEvent::DataChannelOpen,
            error_message: None,
        })
        .unwrap();

    assert!(result.connection.is_none());
    assert_eq!(
        node.session_snapshot(&allocation.session_key)
            .unwrap()
            .channel_attachment,
        WorkerDataChannelAttachmentState::Open
    );
}

#[test]
fn webrtc_acceptor_promotion_queues_through_alpn_gate() {
    let node = node_with_facade_alpn("app/alpn");
    let registration = node.accept_open("app/alpn").unwrap();
    let local = node.endpoint_id();
    let remote = endpoint_id();
    let dial_id = DialId([11; 16]);
    let session_key = WorkerSessionKey::from(dial_id);

    node.handle_bootstrap_signal(WorkerBootstrapSignalInput {
        signal: WebRtcSignal::dial_request_with_alpn(
            dial_id,
            remote,
            local,
            4,
            "",
            BootstrapTransportIntent::WebRtcOnly,
        ),
        alpn: Some("app/alpn".into()),
    })
    .unwrap();
    node.attach_data_channel(&session_key, WorkerDataChannelSource::Received)
        .unwrap();
    let result = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: session_key.clone(),
            event: WorkerMainRtcResultEvent::DataChannelOpen,
            error_message: None,
        })
        .unwrap();

    assert!(result.connection.is_none());
    assert!(node.try_accept_next(registration.id).unwrap().is_none());
    assert_eq!(
        node.session_snapshot(&session_key)
            .unwrap()
            .channel_attachment,
        WorkerDataChannelAttachmentState::Open
    );
}

#[test]
fn webrtc_only_main_failure_yields_no_relay_connection() {
    let node = BrowserWorkerNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();

    let result = node
        .handle_main_rtc_result(WorkerMainRtcResultInput {
            session_key: allocation.session_key.clone(),
            event: WorkerMainRtcResultEvent::WebRtcFailed {
                message: "ice failed".into(),
            },
            error_message: None,
        })
        .unwrap();

    assert!(result.connection.is_none());
    assert_eq!(result.outbound_signals.len(), 1);
    assert_eq!(
        result.outbound_signals[0].terminal_reason(),
        Some(WebRtcTerminalReason::WebRtcFailed)
    );
    assert_error_code(
        node.complete_dial(&allocation.session_key, WorkerResolvedTransport::IrohRelay),
        BrowserWorkerErrorCode::WebRtcFailed,
    );
}

#[tokio::test]
async fn command_dispatch_spawn_and_accept_use_protocol_shapes() {
    let runtime = BrowserWorkerRuntimeCore::new();

    let spawn = runtime
        .handle_command_value(
            WORKER_SPAWN_COMMAND,
            json!({
                "stunUrls": ["stun:stun.example.test:3478"],
                "facadeAlpns": ["app/alpn"]
            }),
        )
        .await
        .unwrap();
    assert!(spawn["nodeKey"].as_str().unwrap().starts_with("node-"));
    assert!(spawn["endpointId"].as_str().unwrap().len() >= 32);
    assert!(spawn.get("node_key").is_none());

    let accept = runtime
        .handle_command_value(WORKER_ACCEPT_OPEN_COMMAND, json!({ "alpn": "app/alpn" }))
        .await
        .unwrap();
    assert_eq!(accept["alpn"], "app/alpn");
    assert!(accept["acceptKey"].as_str().unwrap().starts_with("accept-"));

    let closed = runtime
        .handle_command_value(
            WORKER_ACCEPT_CLOSE_COMMAND,
            json!({ "acceptKey": accept["acceptKey"] }),
        )
        .await
        .unwrap();
    assert_eq!(closed, json!({ "closed": true }));
}

#[tokio::test]
async fn concurrent_spawn_requests_share_runtime_node() {
    let runtime = BrowserWorkerRuntimeCore::new();

    let (first, second) = tokio::join!(
        runtime.handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade()),
        runtime.handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
    );
    let first = first.unwrap();
    let second = second.unwrap();

    assert_eq!(first["nodeKey"], second["nodeKey"]);
    assert_eq!(first["endpointId"], second["endpointId"]);
}

#[tokio::test]
async fn message_dispatch_returns_canonical_response_envelopes() {
    let runtime = BrowserWorkerRuntimeCore::new();

    let spawn = runtime
        .handle_message_value(json!({
            "kind": "request",
            "id": 1,
            "command": WORKER_SPAWN_COMMAND,
            "payload": { "facadeAlpns": ["app/alpn"] }
        }))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(spawn["kind"], "response");
    assert_eq!(spawn["id"], 1);
    assert_eq!(spawn["ok"], true);
    assert!(
        spawn["result"]["nodeKey"]
            .as_str()
            .unwrap()
            .starts_with("node-")
    );
    assert!(spawn["result"].get("node_key").is_none());

    let accept = runtime
        .handle_message_value(json!({
            "kind": "request",
            "id": 2,
            "command": WORKER_ACCEPT_OPEN_COMMAND,
            "payload": { "alpn": "app/alpn" }
        }))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(accept["kind"], "response");
    assert_eq!(accept["id"], 2);
    assert_eq!(accept["ok"], true);
    assert_eq!(accept["result"]["alpn"], "app/alpn");
    assert!(
        accept["result"]["acceptKey"]
            .as_str()
            .unwrap()
            .starts_with("accept-")
    );
}

#[tokio::test]
async fn message_dispatch_turns_command_errors_into_wire_response() {
    let runtime = BrowserWorkerRuntimeCore::new();

    let response = runtime
        .handle_message_value(json!({
            "kind": "request",
            "id": 9,
            "command": WORKER_DIAL_COMMAND,
            "payload": {
                "remoteEndpoint": endpoint_id_string(endpoint_id()),
                "alpn": "app/alpn",
                "transportIntent": "webrtcOnly"
            }
        }))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(response["kind"], "response");
    assert_eq!(response["id"], 9);
    assert_eq!(response["ok"], false);
    assert_eq!(response["error"]["code"], "closed");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("has not been spawned")
    );
}

#[tokio::test]
async fn command_dispatch_accepts_protocol_bootstrap_session_signal() {
    let runtime = BrowserWorkerRuntimeCore::new();
    let spawn = runtime
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    runtime
        .handle_command_value(WORKER_ACCEPT_OPEN_COMMAND, json!({ "alpn": "app/alpn" }))
        .await
        .unwrap();

    let local = EndpointId::from_str(spawn["endpointId"].as_str().unwrap()).unwrap();
    let remote = endpoint_id();
    let dial_id = DialId([7; 16]);
    let session_key = dial_id_string(dial_id);
    let result = runtime
        .handle_command_value(
            WORKER_BOOTSTRAP_SIGNAL_COMMAND,
            json!({
                "signal": {
                    "type": "session",
                    "dialId": session_key,
                    "sessionKey": session_key,
                    "from": endpoint_id_string(remote),
                    "to": endpoint_id_string(local),
                    "generation": 3,
                    "alpn": "app/alpn",
                    "transportIntent": "webrtcOnly"
                }
            }),
        )
        .await
        .unwrap();

    assert_eq!(result["accepted"], true);
    assert_eq!(result["sessionKey"], session_key);
    assert_eq!(result["session"]["transportIntent"], "webrtcOnly");
    assert_eq!(result["mainRtc"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn command_dispatch_consumes_main_rtc_results_and_promotes_webrtc() {
    let runtime = BrowserWorkerRuntimeCore::new();
    runtime
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    let node = runtime.node().unwrap().clone();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();

    let offer = runtime
        .handle_command_value(
            WORKER_MAIN_RTC_RESULT_COMMAND,
            json!({
                "sessionKey": allocation.session_key.as_str(),
                "command": MAIN_RTC_CREATE_OFFER_COMMAND,
                "result": { "sdp": "offer-sdp" }
            }),
        )
        .await
        .unwrap();
    assert_eq!(offer["outboundSignals"][0]["type"], "offer");

    runtime
        .handle_command_value(
            WORKER_ATTACH_DATA_CHANNEL_COMMAND,
            json!({
                "sessionKey": allocation.session_key.as_str(),
                "generation": allocation.generation,
                "source": "created",
                "channel": {}
            }),
        )
        .await
        .unwrap();
    let opened = runtime
        .handle_command_value(
            WORKER_MAIN_RTC_RESULT_COMMAND,
            json!({
                "sessionKey": allocation.session_key.as_str(),
                "event": "dataChannelOpen"
            }),
        )
        .await
        .unwrap();

    assert!(opened["connection"].is_null());
    assert_eq!(opened["session"]["channelAttachment"], "open");
}

#[tokio::test]
async fn command_dispatch_closed_dial_returns_wire_error() {
    let runtime = BrowserWorkerRuntimeCore::new();
    let spawn = runtime
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    runtime
        .handle_command_value(
            WORKER_NODE_CLOSE_COMMAND,
            json!({ "reason": "test shutdown" }),
        )
        .await
        .unwrap();

    assert_wire_error_code(
        runtime
            .handle_command_value(
                WORKER_DIAL_COMMAND,
                json!({
                    "remoteEndpoint": spawn["endpointId"],
                    "alpn": "app/alpn",
                    "transportIntent": "webrtcOnly"
                }),
            )
            .await,
        BrowserWorkerErrorCode::Closed,
    );
}

#[tokio::test]
async fn command_dispatch_relay_dial_and_stream_bridge_use_iroh_connection() {
    let server = BrowserWorkerRuntimeCore::new();
    let client = BrowserWorkerRuntimeCore::new();
    let server_spawn = server
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    client
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    let accept = server
        .handle_command_value(WORKER_ACCEPT_OPEN_COMMAND, json!({ "alpn": "app/alpn" }))
        .await
        .unwrap();
    let direct_addrs = direct_addrs_for(&server.node().unwrap());

    let client_connection = timeout(
        std::time::Duration::from_secs(10),
        client.handle_command_value(
            WORKER_DIAL_COMMAND,
            json!({
                "remoteEndpoint": server_spawn["endpointId"],
                "remoteDirectAddrs": direct_addrs,
                "alpn": "app/alpn",
                "transportIntent": "irohRelay"
            }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(client_connection["transport"], "irohRelay");

    let server_next = timeout(
        std::time::Duration::from_secs(10),
        server.handle_command_value(
            WORKER_ACCEPT_NEXT_COMMAND,
            json!({ "acceptKey": accept["acceptKey"] }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(server_next["done"], false);

    let client_stream = client
        .handle_command_value(
            STREAM_OPEN_BI_COMMAND,
            json!({ "connectionKey": client_connection["connectionKey"] }),
        )
        .await
        .unwrap();
    client
        .handle_command_value(
            STREAM_SEND_CHUNK_COMMAND,
            json!({ "streamKey": client_stream["streamKey"], "chunk": [112, 105, 110, 103] }),
        )
        .await
        .unwrap();
    let server_stream = timeout(
        std::time::Duration::from_secs(10),
        server.handle_command_value(
            STREAM_ACCEPT_BI_COMMAND,
            json!({ "connectionKey": server_next["connection"]["connectionKey"] }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let ping = timeout(
        std::time::Duration::from_secs(10),
        server.handle_command_value(
            STREAM_RECEIVE_CHUNK_COMMAND,
            json!({ "streamKey": server_stream["streamKey"], "maxBytes": 4 }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(ping["done"], false);
    assert_eq!(ping["chunk"], json!([112, 105, 110, 103]));

    server
        .handle_command_value(
            STREAM_SEND_CHUNK_COMMAND,
            json!({ "streamKey": server_stream["streamKey"], "chunk": [112, 111, 110, 103] }),
        )
        .await
        .unwrap();
    let pong = timeout(
        std::time::Duration::from_secs(10),
        client.handle_command_value(
            STREAM_RECEIVE_CHUNK_COMMAND,
            json!({ "streamKey": client_stream["streamKey"], "maxBytes": 4 }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(pong["done"], false);
    assert_eq!(pong["chunk"], json!([112, 111, 110, 103]));
}

#[tokio::test]
async fn command_dispatch_stream_close_send_reaches_remote_receive_done() {
    let server = BrowserWorkerRuntimeCore::new();
    let client = BrowserWorkerRuntimeCore::new();
    let server_spawn = server
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    client
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    let accept = server
        .handle_command_value(WORKER_ACCEPT_OPEN_COMMAND, json!({ "alpn": "app/alpn" }))
        .await
        .unwrap();

    let client_connection = timeout(
        std::time::Duration::from_secs(10),
        client.handle_command_value(
            WORKER_DIAL_COMMAND,
            json!({
                "remoteEndpoint": server_spawn["endpointId"],
                "remoteDirectAddrs": direct_addrs_for(&server.node().unwrap()),
                "alpn": "app/alpn",
                "transportIntent": "irohRelay"
            }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let server_next = timeout(
        std::time::Duration::from_secs(10),
        server.handle_command_value(
            WORKER_ACCEPT_NEXT_COMMAND,
            json!({ "acceptKey": accept["acceptKey"] }),
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let client_stream = client
        .handle_command_value(
            STREAM_OPEN_BI_COMMAND,
            json!({ "connectionKey": client_connection["connectionKey"] }),
        )
        .await
        .unwrap();
    client
        .handle_command_value(
            STREAM_SEND_CHUNK_COMMAND,
            json!({ "streamKey": client_stream["streamKey"], "chunk": [109, 101, 116, 97] }),
        )
        .await
        .unwrap();
    client
        .handle_command_value(
            STREAM_CLOSE_SEND_COMMAND,
            json!({ "streamKey": client_stream["streamKey"] }),
        )
        .await
        .unwrap();

    let server_stream = timeout(
        std::time::Duration::from_secs(10),
        server.handle_command_value(
            STREAM_ACCEPT_BI_COMMAND,
            json!({ "connectionKey": server_next["connection"]["connectionKey"] }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let meta = timeout(
        std::time::Duration::from_secs(10),
        server.handle_command_value(
            STREAM_RECEIVE_CHUNK_COMMAND,
            json!({ "streamKey": server_stream["streamKey"], "maxBytes": 4 }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(meta["done"], false);
    assert_eq!(meta["chunk"], json!([109, 101, 116, 97]));

    let done = timeout(
        std::time::Duration::from_secs(10),
        server.handle_command_value(
            STREAM_RECEIVE_CHUNK_COMMAND,
            json!({ "streamKey": server_stream["streamKey"], "maxBytes": 4 }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(done, json!({ "done": true }));
}

#[tokio::test]
async fn command_dispatch_webrtc_preferred_falls_back_to_relay_connection() {
    let server = BrowserWorkerRuntimeCore::new();
    let client = BrowserWorkerRuntimeCore::new();
    let server_spawn = server
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    client
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    server
        .handle_command_value(WORKER_ACCEPT_OPEN_COMMAND, json!({ "alpn": "app/alpn" }))
        .await
        .unwrap();

    let connection = timeout(
        std::time::Duration::from_secs(10),
        client.handle_command_value(
            WORKER_DIAL_COMMAND,
            json!({
                "remoteEndpoint": server_spawn["endpointId"],
                "remoteDirectAddrs": direct_addrs_for(&server.node().unwrap()),
                "alpn": "app/alpn",
                "transportIntent": "webrtcPreferred"
            }),
        ),
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(connection["transport"], "irohRelay");
    assert!(connection["sessionKey"].as_str().is_some());
}

#[tokio::test]
async fn command_dispatch_webrtc_only_failure_does_not_create_relay_connection() {
    let runtime = BrowserWorkerRuntimeCore::new();
    let spawn = runtime
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();

    assert_wire_error_code(
        runtime
            .handle_command_value(
                WORKER_DIAL_COMMAND,
                json!({
                    "remoteEndpoint": spawn["endpointId"],
                    "alpn": "app/alpn",
                    "transportIntent": "webrtcOnly"
                }),
            )
            .await,
        BrowserWorkerErrorCode::WebRtcFailed,
    );
}

#[tokio::test]
async fn stream_cancel_pending_returns_false_for_unknown_stream() {
    let runtime = BrowserWorkerRuntimeCore::new();
    runtime
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();

    let result = runtime
        .handle_command_value(
            STREAM_CANCEL_PENDING_COMMAND,
            json!({ "requestId": 42, "streamKey": "stream-9", "reason": "caller cancelled" }),
        )
        .await
        .unwrap();

    assert_eq!(result, json!({ "requestId": 42, "cancelled": false }));
}

#[tokio::test]
async fn stream_cancel_pending_wakes_pending_receive() {
    let server = BrowserWorkerRuntimeCore::new();
    let client = BrowserWorkerRuntimeCore::new();
    let server_spawn = server
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    client
        .handle_command_value(WORKER_SPAWN_COMMAND, spawn_payload_with_facade())
        .await
        .unwrap();
    let accept = server
        .handle_command_value(WORKER_ACCEPT_OPEN_COMMAND, json!({ "alpn": "app/alpn" }))
        .await
        .unwrap();

    let client_connection = timeout(
        std::time::Duration::from_secs(10),
        client.handle_command_value(
            WORKER_DIAL_COMMAND,
            json!({
                "remoteEndpoint": server_spawn["endpointId"],
                "remoteDirectAddrs": direct_addrs_for(&server.node().unwrap()),
                "alpn": "app/alpn",
                "transportIntent": "irohRelay"
            }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let server_next = timeout(
        std::time::Duration::from_secs(10),
        server.handle_command_value(
            WORKER_ACCEPT_NEXT_COMMAND,
            json!({ "acceptKey": accept["acceptKey"] }),
        ),
    )
    .await
    .unwrap()
    .unwrap();

    let client_stream = client
        .handle_command_value(
            STREAM_OPEN_BI_COMMAND,
            json!({ "connectionKey": client_connection["connectionKey"] }),
        )
        .await
        .unwrap();
    client
        .handle_command_value(
            STREAM_SEND_CHUNK_COMMAND,
            json!({ "streamKey": client_stream["streamKey"], "chunk": [120] }),
        )
        .await
        .unwrap();
    let server_stream = timeout(
        std::time::Duration::from_secs(10),
        server.handle_command_value(
            STREAM_ACCEPT_BI_COMMAND,
            json!({ "connectionKey": server_next["connection"]["connectionKey"] }),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let seed = server
        .handle_command_value(
            STREAM_RECEIVE_CHUNK_COMMAND,
            json!({ "streamKey": server_stream["streamKey"], "maxBytes": 1 }),
        )
        .await
        .unwrap();
    assert_eq!(seed["done"], false);
    assert_eq!(seed["chunk"], json!([120]));

    let receive = server.handle_command_value(
        STREAM_RECEIVE_CHUNK_COMMAND,
        json!({ "streamKey": server_stream["streamKey"], "maxBytes": 4 }),
    );
    tokio::pin!(receive);
    tokio::select! {
        result = &mut receive => panic!("receive should stay pending without remote data: {result:?}"),
        _ = sleep(std::time::Duration::from_millis(50)) => {}
    }

    let cancel = server
        .handle_command_value(
            STREAM_CANCEL_PENDING_COMMAND,
            json!({ "requestId": 42, "streamKey": server_stream["streamKey"], "reason": "reader cancelled" }),
        )
        .await
        .unwrap();
    assert_eq!(cancel, json!({ "requestId": 42, "cancelled": true }));

    let receive_result = timeout(std::time::Duration::from_secs(1), receive)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receive_result, json!({ "done": true }));

    client
        .handle_command_value(
            STREAM_RESET_COMMAND,
            json!({ "streamKey": client_stream["streamKey"] }),
        )
        .await
        .unwrap();
}
