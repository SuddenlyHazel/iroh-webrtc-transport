use std::{cell::RefCell, rc::Rc};

use crate::core::{
    bootstrap::{BootstrapConfig, BootstrapStream},
    hub::LocalIceEvent,
    signaling::{DialId, WEBRTC_DATA_CHANNEL_LABEL},
};
use iroh::EndpointId;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use super::bootstrap::{
    BootstrapSignalWriter, BrowserBootstrapRuntime, spawn_bootstrap_receiver_loop,
    spawn_bootstrap_signal_writer,
};
use super::js_boundary::{js_error_message, runtime_error_to_js};
use super::rtc_registry::{BrowserRtcControl, attached_rtc_registry};
use super::*;

pub(super) struct PendingBrowserDial {
    sender: oneshot::Sender<BrowserRuntimeResult<BrowserConnectionInfo>>,
    remote_addr: EndpointAddr,
    alpn: String,
}

struct PendingDialCleanup {
    bootstrap: Rc<BrowserBootstrapRuntime>,
    session_key: String,
}

impl PendingDialCleanup {
    fn new(bootstrap: Rc<BrowserBootstrapRuntime>, session_key: String) -> Self {
        Self {
            bootstrap,
            session_key,
        }
    }
}

impl Drop for PendingDialCleanup {
    fn drop(&mut self) {
        self.bootstrap
            .pending_dials
            .borrow_mut()
            .remove(&self.session_key);
        self.bootstrap
            .signal_senders
            .borrow_mut()
            .remove(&self.session_key);
    }
}

pub(super) async fn dial_application_connection(
    core: &Rc<BrowserRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    remote_addr: EndpointAddr,
    alpn: String,
    transport_intent: BootstrapTransportIntent,
) -> BrowserRuntimeResult<BrowserConnectionInfo> {
    let node = core.open_node()?;
    let remote = remote_addr.id;
    if !transport_intent.uses_webrtc() {
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::dial",
            remote = %remote,
            alpn = %alpn,
            transport_intent = ?transport_intent,
            "delegating non-WebRTC dial to core browser runtime"
        );
        return node
            .dial_application_connection(remote_addr, alpn, transport_intent)
            .await;
    }

    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::dial",
        remote = %remote,
        alpn = %alpn,
        transport_intent = ?transport_intent,
        "starting WebRTC bootstrap dial"
    );
    let start = node.allocate_dial_start(remote_addr.id, alpn.clone(), transport_intent)?;
    let session_key = BrowserSessionKey::new(start.session_key.clone())?;
    let session_key_string = session_key.as_str().to_owned();
    let endpoint = node.relay_endpoint()?;
    let bootstrap_stream =
        BootstrapStream::connect(&endpoint, remote_addr.clone(), BootstrapConfig::default())
            .await
            .map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::dial",
                    session_key = %session_key.as_str(),
                    remote = %remote,
                    %err,
                    "failed to open WebRTC bootstrap path"
                );
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::BootstrapFailed,
                    format!("failed to open WebRTC bootstrap path: {err}"),
                )
            })?;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::dial",
        session_key = %session_key.as_str(),
        remote = %remote,
        "opened WebRTC bootstrap path"
    );
    let channel = bootstrap_stream.open_channel().await.map_err(|err| {
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_runtime::dial",
            session_key = %session_key.as_str(),
            remote = %remote,
            %err,
            "failed to open WebRTC bootstrap signal channel"
        );
        BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::BootstrapFailed,
            format!("failed to open WebRTC bootstrap signal channel: {err}"),
        )
    })?;
    let (mut sender, receiver) = channel.split();
    sender
        .send_signal(&start.bootstrap_signal)
        .await
        .map_err(|err| {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::dial",
                session_key = %session_key.as_str(),
                remote = %remote,
                %err,
                "failed to send WebRTC bootstrap dial request"
            );
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::BootstrapFailed,
                format!("failed to send WebRTC bootstrap dial request: {err}"),
            )
        })?;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::dial",
        session_key = %session_key.as_str(),
        remote = %remote,
        "sent WebRTC bootstrap dial request"
    );

    let writer = spawn_bootstrap_signal_writer(sender);
    bootstrap
        .signal_senders
        .borrow_mut()
        .insert(session_key_string.clone(), writer.clone());
    spawn_bootstrap_receiver_loop(
        core.clone(),
        rtc_control.clone(),
        bootstrap.clone(),
        receiver,
        None,
    );

    let (complete_tx, complete_rx) = oneshot::channel();
    bootstrap.pending_dials.borrow_mut().insert(
        session_key_string.clone(),
        PendingBrowserDial {
            sender: complete_tx,
            remote_addr,
            alpn,
        },
    );
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::dial",
        session_key = %session_key.as_str(),
        remote = %remote,
        "registered pending WebRTC dial completion"
    );
    let _pending_dial_cleanup = PendingDialCleanup::new(bootstrap.clone(), session_key_string);

    spawn_direct_dialer_rtc_setup(
        core.clone(),
        rtc_control.clone(),
        bootstrap.clone(),
        writer,
        start,
    );

    let connection = complete_rx.await.map_err(|_| {
        BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::Closed,
            "WebRTC dial completion channel closed",
        )
    })??;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::dial",
        session_key = %session_key.as_str(),
        remote = %remote,
        "completed WebRTC dial"
    );
    Ok(connection)
}

pub(super) fn spawn_direct_dialer_rtc_setup(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    writer: BootstrapSignalWriter,
    start: BrowserDialStart,
) {
    spawn_local(async move {
        let session_key = match BrowserSessionKey::new(start.session_key.clone()) {
            Ok(session_key) => session_key,
            Err(error) => {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::dial",
                    error = %error,
                    "failed to validate direct dialer RTC session key"
                );
                return;
            }
        };
        let registry = match attached_rtc_registry(&rtc_control) {
            Ok(registry) => registry,
            Err(error) => {
                let message = js_error_message(&error);
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::dial",
                    session_key = %session_key.as_str(),
                    error = %message,
                    "failed to start direct dialer RTC setup"
                );
                let _ = complete_direct_dial_setup_failure(
                    &core,
                    &bootstrap,
                    None,
                    session_key,
                    message,
                );
                return;
            }
        };
        if let Err(error) =
            run_direct_dialer_rtc_setup(&core, &rtc_control, &bootstrap, &writer, &registry, &start)
                .await
        {
            let message = js_error_message(&error);
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::dial",
                session_key = %session_key.as_str(),
                error = %message,
                "direct dialer RTC setup failed"
            );
            let _ = complete_direct_dial_setup_failure(
                &core,
                &bootstrap,
                Some(&registry),
                session_key,
                message,
            );
        }
    });
}

async fn run_direct_dialer_rtc_setup(
    core: &Rc<BrowserRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    writer: &BootstrapSignalWriter,
    registry: &crate::browser::BrowserRtcRegistry,
    start: &BrowserDialStart,
) -> Result<(), JsValue> {
    let node = core.open_node().map_err(|err| runtime_error_to_js(err))?;
    let session_config = node.session_config();
    let route = dial_signal_route(&start.bootstrap_signal)?;
    registry
        .create_peer_connection(
            start.session_key.clone(),
            crate::browser::BrowserRtcSessionRole::Dialer,
            start.generation,
            Some(session_config.ice.stun_urls),
            start.remote_endpoint.clone(),
        )
        .map_err(rtc_error_to_js)?;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::dial",
        session_key = %start.session_key,
        "created direct dialer RTC peer connection"
    );

    let channel = registry
        .create_data_channel(
            &start.session_key,
            Some(WEBRTC_DATA_CHANNEL_LABEL),
            Some(WEBRTC_DATA_CHANNEL_LABEL),
            false,
            0,
        )
        .map_err(rtc_error_to_js)?;
    super::data_channel::attach_data_channel_from_main_channel(
        core,
        rtc_control,
        bootstrap,
        start.session_key.clone(),
        DataChannelSource::Created,
        channel,
    )?;

    let sdp = registry
        .create_offer(&start.session_key)
        .await
        .map_err(rtc_error_to_js)?;
    writer.send_signals(vec![WebRtcSignal::Offer {
        dial_id: route.dial_id,
        from: route.from,
        to: route.to,
        generation: route.generation,
        sdp,
    }])?;

    loop {
        let ice = registry
            .next_local_ice(&start.session_key)
            .await
            .map_err(rtc_error_to_js)?;
        let done = matches!(ice, LocalIceEvent::EndOfCandidates);
        writer.send_signals(vec![local_ice_signal(&route, ice)])?;
        if done {
            break;
        }
    }
    Ok(())
}

fn complete_direct_dial_setup_failure(
    core: &Rc<BrowserRuntimeCore>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    registry: Option<&crate::browser::BrowserRtcRegistry>,
    session_key: BrowserSessionKey,
    message: String,
) -> Result<(), JsValue> {
    if let Some(registry) = registry {
        let _ = registry.close_peer_connection(session_key.as_str(), Some(message.clone()));
    }
    let result = core
        .open_node()
        .and_then(|node| {
            node.handle_rtc_lifecycle_result(RtcLifecycleInput {
                session_key,
                event: RtcLifecycleEvent::WebRtcFailed { message },
                error_message: None,
            })
        })
        .map_err(|err| runtime_error_to_js(err))?;
    let _ = super::bootstrap::send_outbound_signals(bootstrap, &result.outbound_signals);
    let dial_result = complete_pending_dial_from_session(
        core.clone(),
        bootstrap.clone(),
        Some(result.session_key.as_str()),
        result.session.as_ref(),
        super::bootstrap::terminal_message(&result.outbound_signals),
    );
    let prepare_result =
        super::protocol_transport::complete_pending_protocol_transport_prepare_from_result(
            core.clone(),
            bootstrap.clone(),
            Some(result.session_key.as_str()),
            result.session.as_ref(),
        );
    dial_result?;
    prepare_result
}

#[derive(Debug, Clone, Copy)]
struct DialSignalRoute {
    dial_id: DialId,
    from: EndpointId,
    to: EndpointId,
    generation: u64,
}

fn dial_signal_route(signal: &WebRtcSignal) -> Result<DialSignalRoute, JsValue> {
    let WebRtcSignal::DialRequest {
        dial_id,
        from,
        to,
        generation,
        ..
    } = signal
    else {
        return Err(JsValue::from_str(
            "direct dialer RTC setup requires a dial request bootstrap signal",
        ));
    };
    Ok(DialSignalRoute {
        dial_id: *dial_id,
        from: *from,
        to: *to,
        generation: *generation,
    })
}

fn local_ice_signal(route: &DialSignalRoute, ice: LocalIceEvent) -> WebRtcSignal {
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

pub(super) fn complete_pending_dial_from_session(
    core: Rc<BrowserRuntimeCore>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    session_key: Option<&str>,
    session: Option<&BrowserSessionSnapshot>,
    diagnostic: Option<String>,
) -> Result<(), JsValue> {
    let Some(session_key) = session_key else {
        return Ok(());
    };
    let Some(session) = session else {
        return Ok(());
    };
    let action = pending_dial_action_from_session(session);
    let Some(action) = action else {
        return Ok(());
    };
    let pending = bootstrap.pending_dials.borrow_mut().remove(session_key);
    let Some(pending) = pending else {
        return Ok(());
    };
    let session_key =
        BrowserSessionKey::new(session_key.to_owned()).map_err(|err| runtime_error_to_js(err))?;
    match action {
        PendingDialAction::WebRtc => {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_runtime::dial",
                session_key = %session_key.as_str(),
                "pending dial resolved to WebRTC custom transport"
            );
            spawn_local(async move {
                let result = match core.open_node() {
                    Ok(node) => node.dial_webrtc_application_connection(&session_key).await,
                    Err(err) => Err(err),
                };
                let _ = pending.sender.send(result);
            });
        }
        PendingDialAction::Relay => {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_runtime::dial",
                session_key = %session_key.as_str(),
                "pending dial resolved to Iroh relay"
            );
            spawn_local(async move {
                let result = match core.open_node() {
                    Ok(node) => {
                        node.dial_iroh_application_connection(
                            pending.remote_addr,
                            pending.alpn,
                            Some(session_key),
                        )
                        .await
                    }
                    Err(err) => Err(err),
                };
                let _ = pending.sender.send(result);
            });
        }
        PendingDialAction::Fail => {
            let message = diagnostic.unwrap_or_else(|| "WebRTC dial failed".to_owned());
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::dial",
                session_key = %session_key.as_str(),
                diagnostic = %message,
                "pending dial failed"
            );
            let _ = pending.sender.send(Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                message,
            )));
        }
        PendingDialAction::Closed => {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_runtime::dial",
                session_key = %session_key.as_str(),
                "pending dial closed"
            );
            let _ = pending.sender.send(Err(BrowserRuntimeError::closed()));
        }
    }
    Ok(())
}

enum PendingDialAction {
    WebRtc,
    Relay,
    Fail,
    Closed,
}

fn pending_dial_action_from_session(session: &BrowserSessionSnapshot) -> Option<PendingDialAction> {
    match (
        session.resolved_transport,
        session.channel_attachment,
        session.lifecycle,
    ) {
        (_, DataChannelAttachmentState::Open, _) => Some(PendingDialAction::WebRtc),
        (Some(BrowserResolvedTransport::IrohRelay), _, _)
        | (_, _, BrowserSessionLifecycle::RelaySelected) => Some(PendingDialAction::Relay),
        (_, _, BrowserSessionLifecycle::Failed) | (_, DataChannelAttachmentState::Failed, _) => {
            Some(PendingDialAction::Fail)
        }
        (_, _, BrowserSessionLifecycle::Closed) | (_, DataChannelAttachmentState::Closed, _) => {
            Some(PendingDialAction::Closed)
        }
        _ => None,
    }
}
