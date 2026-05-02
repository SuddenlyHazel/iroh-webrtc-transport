use super::*;
use n0_future::{task, time::timeout};
use serde_json::json;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex as StdMutex},
};

fn node_with_accept_capacity(capacity: usize) -> BrowserRuntimeNode {
    BrowserRuntimeNode::spawn(BrowserRuntimeNodeConfig {
        accept_queue_capacity: capacity,
        facade_alpns: vec!["app/alpn".into()],
        ..BrowserRuntimeNodeConfig::default()
    })
    .unwrap()
}

fn node_with_facade_alpn(alpn: &str) -> BrowserRuntimeNode {
    BrowserRuntimeNode::spawn(BrowserRuntimeNodeConfig {
        facade_alpns: vec![alpn.into()],
        ..BrowserRuntimeNodeConfig::default()
    })
    .unwrap()
}

fn node_with_benchmark_alpn(alpn: &str) -> BrowserRuntimeNode {
    BrowserRuntimeNode::spawn(BrowserRuntimeNodeConfig {
        benchmark_echo_alpns: vec![alpn.into()],
        ..BrowserRuntimeNodeConfig::default()
    })
    .unwrap()
}

fn endpoint_id() -> EndpointId {
    SecretKey::generate().public()
}

fn assert_error_code<T: std::fmt::Debug>(
    result: BrowserRuntimeResult<T>,
    code: BrowserRuntimeErrorCode,
) -> BrowserRuntimeError {
    let err = result.expect_err("operation should fail");
    assert_eq!(err.code, code);
    err
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

impl crate::browser::BrowserProtocol for TestProtocol {
    const ALPN: &'static [u8] = b"test/browser-protocol/1";

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
async fn typed_browser_protocol_registry_round_trips_command_and_event() {
    let protocol = TestProtocol::default();
    protocol
        .events
        .lock()
        .expect("test protocol mutex poisoned")
        .push_back(TestProtocolEvent::Joined {
            topic: "rust".into(),
        });
    let mut registry = BrowserProtocolRegistry::new();
    registry.register(protocol.clone()).unwrap();

    let command = json!({ "Join": { "topic": "rust" } });
    let result = registry
        .handle_command("test/browser-protocol/1", command)
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
        .next_event("test/browser-protocol/1")
        .await
        .unwrap();
    assert_eq!(event, json!({ "Joined": { "topic": "rust" } }));
}

#[tokio::test]
async fn browser_protocol_endpoint_connect_rejects_relay_when_webrtc_only_cannot_prepare() {
    let mut registry = BrowserProtocolRegistry::new();
    registry.register(TestProtocol::default()).unwrap();
    let node = BrowserRuntimeNode::spawn_with_secret_key(
        BrowserRuntimeNodeConfig {
            protocol_transport_intent: BootstrapTransportIntent::WebRtcOnly,
            ..BrowserRuntimeNodeConfig::default()
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
            <TestProtocol as crate::browser::BrowserProtocol>::ALPN,
        ),
    )
    .await
    .expect("WebRTC-only browser protocol connect should reject without hanging");

    let error = result.unwrap_err().to_string();
    assert!(
        error.to_ascii_lowercase().contains("rejected locally"),
        "unexpected connect error: {error}"
    );
}

#[tokio::test]
async fn browser_protocol_endpoint_connect_requests_webrtc_prepare_before_resolution() {
    let mut registry = BrowserProtocolRegistry::new();
    registry.register(TestProtocol::default()).unwrap();
    let (prepare_tx, mut prepare_rx) = mpsc::channel(1);
    let node = BrowserRuntimeNode::spawn_with_secret_key(
        BrowserRuntimeNodeConfig {
            protocol_transport_intent: BootstrapTransportIntent::WebRtcOnly,
            protocol_transport_prepare_tx: Some(prepare_tx),
            ..BrowserRuntimeNodeConfig::default()
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
        <TestProtocol as crate::browser::BrowserProtocol>::ALPN,
    );
    tokio::pin!(connect);

    let request = tokio::select! {
        request = prepare_rx.recv() => request.expect("prepare request should be sent"),
        result = &mut connect => panic!("connect finished before transport preparation: {result:?}"),
    };

    assert_eq!(request.remote, remote);
    assert_eq!(
        request.alpn.as_bytes(),
        <TestProtocol as crate::browser::BrowserProtocol>::ALPN
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
        .expect("prepared custom address should be stored in browser protocol lookup");

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
fn browser_protocol_registry_rejects_duplicate_protocol_type_and_alpn() {
    let mut registry = BrowserProtocolRegistry::new();
    registry.register(TestProtocol::default()).unwrap();
    assert!(registry.register(TestProtocol::default()).is_err());
}

#[test]
fn duplicate_accept_registration_is_rejected() {
    let node = node_with_facade_alpn("app/alpn");
    let first = node.accept_open("app/alpn").unwrap();

    assert_eq!(first.alpn, "app/alpn");
    assert_error_code(
        node.accept_open("app/alpn"),
        BrowserRuntimeErrorCode::UnsupportedAlpn,
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
        BrowserResolvedTransport::IrohRelay,
        None,
    )
    .unwrap();
    assert_error_code(
        node.admit_incoming_application_connection(
            remote,
            "app/alpn",
            BrowserResolvedTransport::IrohRelay,
            None,
        ),
        BrowserRuntimeErrorCode::WebRtcFailed,
    );

    let next = node.accept_next(registration.id).await.unwrap();
    match next {
        BrowserAcceptNext::Ready(connection) => {
            assert_eq!(connection.alpn, "app/alpn");
            assert_eq!(connection.remote, remote);
        }
        BrowserAcceptNext::Done => panic!("queued connection should be delivered"),
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
    assert_eq!(next, BrowserAcceptNext::Done);
    assert!(!node.is_alpn_registered("app/alpn"));
}

#[test]
fn inactive_alpn_is_rejected_for_incoming_connections() {
    let node = BrowserRuntimeNode::spawn_default().unwrap();

    assert_error_code(
        node.admit_incoming_application_connection(
            endpoint_id(),
            "inactive/alpn",
            BrowserResolvedTransport::IrohRelay,
            None,
        ),
        BrowserRuntimeErrorCode::UnsupportedAlpn,
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
        .handle_bootstrap_signal(BrowserBootstrapSignalInput { signal, alpn: None })
        .unwrap();

    assert!(result.accepted);
    assert_eq!(result.session.unwrap().alpn, "bench/alpn");
}

#[test]
fn simultaneous_same_remote_alpn_dials_allocate_independent_sessions() {
    let node = BrowserRuntimeNode::spawn_default().unwrap();
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
    let node = BrowserRuntimeNode::spawn_default().unwrap();
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
        BrowserRuntimeErrorCode::Closed,
    );
    assert_error_code(
        node.complete_dial(&allocation.session_key, BrowserResolvedTransport::IrohRelay),
        BrowserRuntimeErrorCode::Closed,
    );
    let snapshot = node.session_snapshot(&allocation.session_key).unwrap();
    assert_eq!(snapshot.lifecycle, BrowserSessionLifecycle::Closed);
}

#[test]
fn endpoint_id_remains_cached_after_close() {
    let node = BrowserRuntimeNode::spawn_default().unwrap();
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
    let node = BrowserRuntimeNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();

    assert_error_code(
        node.complete_dial(&allocation.session_key, BrowserResolvedTransport::WebRtc),
        BrowserRuntimeErrorCode::DataChannelFailed,
    );

    let attached = node
        .attach_transferred_channel(&allocation.session_key)
        .unwrap();
    assert_eq!(
        attached.channel_attachment,
        DataChannelAttachmentState::Transferred
    );

    let opened = node
        .mark_transferred_channel_open(&allocation.session_key)
        .unwrap();
    assert_eq!(opened.channel_attachment, DataChannelAttachmentState::Open);

    let complete = node
        .complete_dial(&allocation.session_key, BrowserResolvedTransport::WebRtc)
        .unwrap();
    assert_eq!(
        complete.resolved_transport,
        Some(BrowserResolvedTransport::WebRtc)
    );
    assert_eq!(
        complete.lifecycle,
        BrowserSessionLifecycle::ApplicationReady
    );
}

#[test]
fn dial_completion_enforces_transport_preference() {
    let node = BrowserRuntimeNode::spawn_default().unwrap();
    let remote = endpoint_id();
    let relay_only = node
        .allocate_dial(remote, "app/alpn", BootstrapTransportIntent::IrohRelay)
        .unwrap();
    let webrtc_only = node
        .allocate_dial(remote, "app/alpn", BootstrapTransportIntent::WebRtcOnly)
        .unwrap();

    assert_error_code(
        node.complete_dial(&relay_only.session_key, BrowserResolvedTransport::WebRtc),
        BrowserRuntimeErrorCode::WebRtcFailed,
    );
    assert_error_code(
        node.complete_dial(
            &webrtc_only.session_key,
            BrowserResolvedTransport::IrohRelay,
        ),
        BrowserRuntimeErrorCode::WebRtcFailed,
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
        .handle_bootstrap_signal(BrowserBootstrapSignalInput {
            signal,
            alpn: Some("app/alpn".into()),
        })
        .unwrap();

    let session = result.session.unwrap();
    assert_eq!(
        session.transport_intent,
        BootstrapTransportIntent::WebRtcOnly
    );
    assert_eq!(session.role, BrowserSessionRole::Acceptor);
}

#[test]
fn terminal_decisions_follow_transport_intent() {
    let node = BrowserRuntimeNode::spawn_default().unwrap();
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
        Some(BrowserResolvedTransport::IrohRelay)
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
        Some(BrowserResolvedTransport::IrohRelay)
    );
    assert_eq!(
        preferred_snapshot.channel_attachment,
        DataChannelAttachmentState::Failed
    );
    assert_eq!(
        preferred_snapshot.lifecycle,
        BrowserSessionLifecycle::RelaySelected
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
        DataChannelAttachmentState::Failed
    );
    assert_eq!(only_snapshot.lifecycle, BrowserSessionLifecycle::Failed);
}

#[test]
fn remote_terminal_fallback_updates_session_as_single_transition() {
    let node = BrowserRuntimeNode::spawn_default().unwrap();
    let remote = endpoint_id();
    let allocation = node
        .allocate_dial(remote, "app/alpn", BootstrapTransportIntent::WebRtcOnly)
        .unwrap();

    let result = node
        .handle_bootstrap_signal(BrowserBootstrapSignalInput {
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
        DataChannelAttachmentState::Failed
    );
    assert_eq!(snapshot.lifecycle, BrowserSessionLifecycle::Failed);
}

#[test]
fn cancel_decision_emits_terminal_cancel() {
    let node = BrowserRuntimeNode::spawn_default().unwrap();
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
        BrowserSessionLifecycle::Closed
    );
}

#[test]
fn remote_bootstrap_signals_do_not_emit_rtc_actions() {
    let node = node_with_facade_alpn("app/alpn");
    let local = node.endpoint_id();
    let remote = endpoint_id();
    node.accept_open("app/alpn").unwrap();
    let dial_id = DialId([9; 16]);
    node.handle_bootstrap_signal(BrowserBootstrapSignalInput {
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
        .handle_bootstrap_signal(BrowserBootstrapSignalInput {
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
    assert!(offer.accepted);

    let ice = node
        .handle_bootstrap_signal(BrowserBootstrapSignalInput {
            signal: WebRtcSignal::IceCandidate {
                dial_id,
                from: remote,
                to: local,
                generation: 1,
                candidate: "candidate:1".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: Some(0),
            },
            alpn: None,
        })
        .unwrap();
    assert!(ice.accepted);

    let end = node
        .handle_bootstrap_signal(BrowserBootstrapSignalInput {
            signal: WebRtcSignal::EndOfCandidates {
                dial_id,
                from: remote,
                to: local,
                generation: 1,
            },
            alpn: None,
        })
        .unwrap();
    assert!(end.accepted);

    let dial = node
        .allocate_dial(remote, "app/alpn", BootstrapTransportIntent::WebRtcOnly)
        .unwrap();
    let answer = node
        .handle_bootstrap_signal(BrowserBootstrapSignalInput {
            signal: WebRtcSignal::Answer {
                dial_id: dial.dial_id,
                from: remote,
                to: dial.local,
                generation: dial.generation,
                sdp: "answer-sdp".into(),
            },
            alpn: None,
        })
        .unwrap();
    assert!(answer.accepted);
}

#[test]
fn webrtc_dialer_promotes_only_after_transferred_channel_opens() {
    let node = BrowserRuntimeNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();

    assert_error_code(
        node.handle_rtc_lifecycle_result(RtcLifecycleInput {
            session_key: allocation.session_key.clone(),
            event: RtcLifecycleEvent::DataChannelOpen,
            error_message: None,
        }),
        BrowserRuntimeErrorCode::DataChannelFailed,
    );

    node.attach_data_channel(&allocation.session_key, DataChannelSource::Created)
        .unwrap();
    let result = node
        .handle_rtc_lifecycle_result(RtcLifecycleInput {
            session_key: allocation.session_key.clone(),
            event: RtcLifecycleEvent::DataChannelOpen,
            error_message: None,
        })
        .unwrap();

    assert!(result.connection.is_none());
    assert_eq!(
        node.session_snapshot(&allocation.session_key)
            .unwrap()
            .channel_attachment,
        DataChannelAttachmentState::Open
    );
}

#[test]
fn webrtc_acceptor_promotion_queues_through_alpn_gate() {
    let node = node_with_facade_alpn("app/alpn");
    let registration = node.accept_open("app/alpn").unwrap();
    let local = node.endpoint_id();
    let remote = endpoint_id();
    let dial_id = DialId([11; 16]);
    let session_key = BrowserSessionKey::from(dial_id);

    node.handle_bootstrap_signal(BrowserBootstrapSignalInput {
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
    node.attach_data_channel(&session_key, DataChannelSource::Received)
        .unwrap();
    let result = node
        .handle_rtc_lifecycle_result(RtcLifecycleInput {
            session_key: session_key.clone(),
            event: RtcLifecycleEvent::DataChannelOpen,
            error_message: None,
        })
        .unwrap();

    assert!(result.connection.is_none());
    assert!(node.try_accept_next(registration.id).unwrap().is_none());
    assert_eq!(
        node.session_snapshot(&session_key)
            .unwrap()
            .channel_attachment,
        DataChannelAttachmentState::Open
    );
}

#[test]
fn webrtc_only_main_failure_yields_no_relay_connection() {
    let node = BrowserRuntimeNode::spawn_default().unwrap();
    let allocation = node
        .allocate_dial(
            endpoint_id(),
            "app/alpn",
            BootstrapTransportIntent::WebRtcOnly,
        )
        .unwrap();

    let result = node
        .handle_rtc_lifecycle_result(RtcLifecycleInput {
            session_key: allocation.session_key.clone(),
            event: RtcLifecycleEvent::WebRtcFailed {
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
        node.complete_dial(&allocation.session_key, BrowserResolvedTransport::IrohRelay),
        BrowserRuntimeErrorCode::WebRtcFailed,
    );
}
