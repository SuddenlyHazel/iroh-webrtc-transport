use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use crate::core::{
    bootstrap::{BootstrapConfig, BootstrapSignalSender, BootstrapStream},
    hub::{LocalIceEvent, WebRtcIceCandidate},
};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use super::dial::{PendingBrowserDial, complete_pending_dial_from_session};
use super::js_boundary::{js_error_message, runtime_error_to_js};
use super::protocol_transport::{
    PendingProtocolTransportPrepare, complete_pending_protocol_transport_prepare_from_result,
};
use super::rtc_registry::{BrowserRtcControl, attached_rtc_registry};
use super::*;

pub(super) struct BrowserBootstrapRuntime {
    pub(super) connection_tx: mpsc::UnboundedSender<Connection>,
    connection_rx: RefCell<Option<mpsc::UnboundedReceiver<Connection>>>,
    pub(super) signal_senders: RefCell<HashMap<String, BootstrapSignalWriter>>,
    pub(super) pending_dials: RefCell<HashMap<String, PendingBrowserDial>>,
    pub(super) pending_protocol_transport_prepares:
        RefCell<HashMap<String, PendingProtocolTransportPrepare>>,
    accept_loop_started: Cell<bool>,
}

#[derive(Clone)]
pub(super) struct BootstrapSignalWriter {
    tx: mpsc::UnboundedSender<BootstrapSignalBatch>,
}

struct BootstrapSignalBatch {
    signals: Vec<WebRtcSignal>,
}

impl BootstrapSignalWriter {
    pub(super) fn send_signals(&self, signals: Vec<WebRtcSignal>) -> Result<(), JsValue> {
        if signals.is_empty() {
            return Ok(());
        }
        self.tx
            .send(BootstrapSignalBatch { signals })
            .map_err(|_| JsValue::from_str("bootstrap signal writer is closed"))
    }
}

impl BrowserBootstrapRuntime {
    pub(super) fn new() -> Self {
        let (connection_tx, connection_rx) = mpsc::unbounded_channel();
        Self {
            connection_tx,
            connection_rx: RefCell::new(Some(connection_rx)),
            signal_senders: RefCell::new(HashMap::new()),
            pending_dials: RefCell::new(HashMap::new()),
            pending_protocol_transport_prepares: RefCell::new(HashMap::new()),
            accept_loop_started: Cell::new(false),
        }
    }
}

pub(super) fn start_bootstrap_accept_loop(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
) {
    if bootstrap.accept_loop_started.replace(true) {
        return;
    }
    let Some(mut receiver) = bootstrap.connection_rx.borrow_mut().take() else {
        return;
    };
    spawn_local(async move {
        while let Some(connection) = receiver.recv().await {
            let core = core.clone();
            let rtc_control = rtc_control.clone();
            let bootstrap = bootstrap.clone();
            spawn_local(async move {
                handle_incoming_bootstrap_connection(core, rtc_control, bootstrap, connection)
                    .await;
            });
        }
    });
}

async fn handle_incoming_bootstrap_connection(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    connection: Connection,
) {
    let stream = BootstrapStream::from_connection(connection, BootstrapConfig::default());
    let channel = match stream.accept_channel().await {
        Ok(channel) => channel,
        Err(_) => return,
    };
    let (sender, receiver) = channel.split();
    let writer = spawn_bootstrap_signal_writer(sender);
    spawn_bootstrap_receiver_loop_with_sender(
        core,
        rtc_control,
        bootstrap,
        receiver,
        Some(writer),
        None,
    );
}

pub(super) fn spawn_bootstrap_receiver_loop(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    receiver: crate::core::bootstrap::BootstrapSignalReceiver,
    alpn: Option<String>,
) {
    spawn_bootstrap_receiver_loop_with_sender(core, rtc_control, bootstrap, receiver, None, alpn);
}

fn spawn_bootstrap_receiver_loop_with_sender(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    mut receiver: crate::core::bootstrap::BootstrapSignalReceiver,
    sender: Option<BootstrapSignalWriter>,
    alpn: Option<String>,
) {
    spawn_local(async move {
        loop {
            let signal = match receiver.recv_signal().await {
                Ok(signal) => signal,
                Err(_) => return,
            };
            let input = BrowserBootstrapSignalInput {
                signal: signal.clone(),
                alpn: alpn.clone(),
            };
            let node = match core.open_node() {
                Ok(node) => node,
                Err(_) => return,
            };
            let result = match node.handle_bootstrap_signal(input) {
                Ok(result) => result,
                Err(_) => return,
            };
            if let (Some(sender), Some(session_key)) =
                (sender.as_ref(), result.session_key.as_ref())
            {
                bootstrap
                    .signal_senders
                    .borrow_mut()
                    .entry(session_key.clone())
                    .or_insert_with(|| sender.clone());
            }
            if handle_direct_bootstrap_rtc_signal(&core, &rtc_control, &bootstrap, &signal, &result)
                .await
                .is_err()
            {
                return;
            }
            let _ = send_outbound_signals(&bootstrap, &result.outbound_signals);
            let _ = complete_pending_dial_from_session(
                core.clone(),
                bootstrap.clone(),
                result.session_key.as_deref(),
                result.session.as_ref(),
                terminal_message(&result.outbound_signals),
            );
            let _ = complete_pending_protocol_transport_prepare_from_result(
                core.clone(),
                bootstrap.clone(),
                result.session_key.as_deref(),
                result.session.as_ref(),
            );
        }
    });
}

async fn handle_direct_bootstrap_rtc_signal(
    core: &Rc<BrowserRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    signal: &WebRtcSignal,
    result: &BrowserBootstrapSignalResult,
) -> Result<(), JsValue> {
    let Some(session_key) = result.session_key.as_deref() else {
        return Ok(());
    };
    match signal {
        WebRtcSignal::DialRequest {
            from,
            generation,
            transport_intent,
            ..
        } => {
            if !transport_intent.uses_webrtc() {
                return Ok(());
            }
            let node = core.open_node().map_err(|err| runtime_error_to_js(err))?;
            let session_config = node.session_config();
            let registry = attached_rtc_registry(rtc_control)?;
            registry
                .create_peer_connection(
                    session_key.to_owned(),
                    crate::browser::BrowserRtcSessionRole::Acceptor,
                    *generation,
                    Some(session_config.ice.stun_urls),
                    endpoint_id_string(*from),
                )
                .map_err(rtc_error_to_js)?;
        }
        WebRtcSignal::Offer {
            dial_id,
            from,
            to,
            generation,
            sdp,
        } => {
            let registry = attached_rtc_registry(rtc_control)?;
            registry
                .accept_offer(session_key, Some(endpoint_id_string(*from)), sdp)
                .await
                .map_err(rtc_error_to_js)?;
            let answer_sdp = registry
                .create_answer(session_key)
                .await
                .map_err(rtc_error_to_js)?;
            let route = BootstrapSignalRoute {
                dial_id: *dial_id,
                from: *to,
                to: *from,
                generation: *generation,
            };
            send_signals_for_session(
                bootstrap,
                session_key,
                vec![WebRtcSignal::Answer {
                    dial_id: route.dial_id,
                    from: route.from,
                    to: route.to,
                    generation: route.generation,
                    sdp: answer_sdp,
                }],
            )?;
            spawn_acceptor_data_channel_attach(
                core.clone(),
                rtc_control.clone(),
                bootstrap.clone(),
                registry.clone(),
                session_key.to_owned(),
            );
            spawn_acceptor_local_ice_drain(
                bootstrap.clone(),
                registry,
                session_key.to_owned(),
                route,
            );
        }
        WebRtcSignal::Answer { sdp, .. } => {
            let registry = attached_rtc_registry(rtc_control)?;
            registry
                .accept_answer(session_key, sdp)
                .await
                .map_err(rtc_error_to_js)?;
        }
        WebRtcSignal::IceCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
            ..
        } => {
            let registry = attached_rtc_registry(rtc_control)?;
            registry
                .add_remote_ice(
                    session_key,
                    LocalIceEvent::Candidate(WebRtcIceCandidate {
                        candidate: candidate.clone(),
                        sdp_mid: sdp_mid.clone(),
                        sdp_mline_index: *sdp_mline_index,
                    }),
                )
                .await
                .map_err(rtc_error_to_js)?;
        }
        WebRtcSignal::EndOfCandidates { .. } => {
            let registry = attached_rtc_registry(rtc_control)?;
            registry
                .add_remote_ice(session_key, LocalIceEvent::EndOfCandidates)
                .await
                .map_err(rtc_error_to_js)?;
        }
        _ => {}
    }
    Ok(())
}

fn spawn_acceptor_data_channel_attach(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    registry: crate::browser::BrowserRtcRegistry,
    session_key: String,
) {
    spawn_local(async move {
        let channel = match registry.take_data_channel(&session_key).await {
            Ok(channel) => channel,
            Err(error) => {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::bootstrap",
                    session_key = %session_key,
                    %error,
                    "failed to receive acceptor RTCDataChannel"
                );
                return;
            }
        };
        if let Err(error) = super::data_channel::attach_data_channel_from_main_channel(
            &core,
            &rtc_control,
            &bootstrap,
            session_key.clone(),
            DataChannelSource::Received,
            channel,
        ) {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::bootstrap",
                session_key = %session_key,
                error = %js_error_message(&error),
                "failed to attach acceptor RTCDataChannel"
            );
        }
    });
}

fn spawn_acceptor_local_ice_drain(
    bootstrap: Rc<BrowserBootstrapRuntime>,
    registry: crate::browser::BrowserRtcRegistry,
    session_key: String,
    route: BootstrapSignalRoute,
) {
    spawn_local(async move {
        if let Err(error) = drain_local_ice(&bootstrap, &registry, &session_key, route).await {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::bootstrap",
                session_key = %session_key,
                error = %js_error_message(&error),
                "failed to drain acceptor local ICE"
            );
        }
    });
}

#[derive(Debug, Clone, Copy)]
struct BootstrapSignalRoute {
    dial_id: DialId,
    from: EndpointId,
    to: EndpointId,
    generation: u64,
}

async fn drain_local_ice(
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    registry: &crate::browser::BrowserRtcRegistry,
    session_key: &str,
    route: BootstrapSignalRoute,
) -> Result<(), JsValue> {
    loop {
        let ice = registry
            .next_local_ice(session_key)
            .await
            .map_err(rtc_error_to_js)?;
        let done = matches!(ice, LocalIceEvent::EndOfCandidates);
        send_signals_for_session(bootstrap, session_key, vec![local_ice_signal(&route, ice)])?;
        if done {
            break;
        }
    }
    Ok(())
}

fn send_signals_for_session(
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    session_key: &str,
    signals: Vec<WebRtcSignal>,
) -> Result<(), JsValue> {
    let sender = bootstrap.signal_senders.borrow().get(session_key).cloned();
    let Some(sender) = sender else {
        return Err(JsValue::from_str(&format!(
            "missing bootstrap signal sender for WebRTC session {session_key}"
        )));
    };
    sender.send_signals(signals)
}

fn local_ice_signal(route: &BootstrapSignalRoute, ice: LocalIceEvent) -> WebRtcSignal {
    match ice {
        LocalIceEvent::Candidate(candidate) => WebRtcSignal::IceCandidate {
            dial_id: route.dial_id,
            from: route.from,
            to: route.to,
            generation: route.generation,
            candidate: candidate.candidate,
            sdp_mid: candidate.sdp_mid,
            sdp_mline_index: candidate.sdp_mline_index,
        },
        LocalIceEvent::EndOfCandidates => WebRtcSignal::EndOfCandidates {
            dial_id: route.dial_id,
            from: route.from,
            to: route.to,
            generation: route.generation,
        },
    }
}

fn rtc_error_to_js(error: crate::error::Error) -> JsValue {
    JsValue::from_str(&error.to_string())
}

pub(super) fn send_outbound_signals(
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    signals: &[WebRtcSignal],
) -> Result<(), JsValue> {
    if signals.is_empty() {
        return Ok(());
    }
    let mut by_session: HashMap<String, Vec<WebRtcSignal>> = HashMap::new();
    for signal in signals {
        by_session
            .entry(
                BrowserSessionKey::from(signal.dial_id())
                    .as_str()
                    .to_owned(),
            )
            .or_default()
            .push(signal.clone());
    }
    for (session_key, signals) in by_session {
        let sender = bootstrap.signal_senders.borrow().get(&session_key).cloned();
        let Some(sender) = sender else {
            let message =
                format!("missing bootstrap signal sender for WebRTC session {session_key}");
            return Err(runtime_error_to_js(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::BootstrapFailed,
                message,
            )));
        };
        let _ = sender.tx.send(BootstrapSignalBatch { signals });
    }
    Ok(())
}

pub(super) fn terminal_message(signals: &[WebRtcSignal]) -> Option<String> {
    signals.iter().find_map(|signal| match signal {
        WebRtcSignal::Terminal { message, .. } => message.clone(),
        _ => None,
    })
}

pub(super) fn spawn_bootstrap_signal_writer(
    mut sender: BootstrapSignalSender,
) -> BootstrapSignalWriter {
    let (tx, mut rx) = mpsc::unbounded_channel::<BootstrapSignalBatch>();
    spawn_local(async move {
        while let Some(batch) = rx.recv().await {
            for signal in batch.signals {
                let terminal = signal.is_terminal();
                if sender.send_signal(&signal).await.is_err() {
                    return;
                }
                if terminal {
                    let _ = sender.finish();
                    return;
                }
            }
        }
    });
    BootstrapSignalWriter { tx }
}
