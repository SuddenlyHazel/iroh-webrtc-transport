use std::{cell::RefCell, rc::Rc};

use crate::core::bootstrap::{BootstrapConfig, BootstrapStream};
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use super::bootstrap::{
    WorkerBootstrapRuntime, spawn_bootstrap_receiver_loop, spawn_bootstrap_signal_writer,
};
use super::rtc_control::{WorkerRtcControlPort, dispatch_main_rtc_commands_from_result};
use super::wire::{js_error_message, wire_error_to_js};
use super::*;

pub(super) struct PendingProtocolTransportPrepare {
    pub(super) sender: oneshot::Sender<BrowserWorkerResult<()>>,
    pub(super) remote: EndpointId,
    pub(super) transport_intent: BootstrapTransportIntent,
}

pub(super) fn start_protocol_transport_prepare_loop(
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
    post_message: Option<js_sys::Function>,
    mut receiver: mpsc::Receiver<WorkerProtocolTransportPrepareRequest>,
) {
    spawn_local(async move {
        while let Some(request) = receiver.recv().await {
            let core = core.clone();
            let rtc_control = rtc_control.clone();
            let bootstrap = bootstrap.clone();
            let post_message = post_message.clone();
            spawn_local(async move {
                prepare_protocol_transport(
                    core,
                    rtc_control,
                    bootstrap,
                    post_message.as_ref(),
                    request,
                )
                .await;
            });
        }
    });
}

async fn prepare_protocol_transport(
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
    post_message: Option<&js_sys::Function>,
    request: WorkerProtocolTransportPrepareRequest,
) {
    let result = prepare_protocol_transport_inner(
        &core,
        &rtc_control,
        &bootstrap,
        post_message,
        request.remote,
        request.alpn,
        request.transport_intent,
    )
    .await;
    let _ = request.response.send(result);
}

async fn prepare_protocol_transport_inner(
    core: &Rc<BrowserWorkerRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: &Rc<WorkerBootstrapRuntime>,
    post_message: Option<&js_sys::Function>,
    remote: EndpointId,
    alpn: String,
    transport_intent: BootstrapTransportIntent,
) -> BrowserWorkerResult<()> {
    let node = core.open_node()?;
    let remote_addr = EndpointAddr::new(remote);
    let start = node.allocate_dial_start(remote, alpn, transport_intent)?;
    let session_key = WorkerSessionKey::new(start.session_key.clone())?;
    let session_key_string = session_key.as_str().to_owned();
    let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await
        .map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::BootstrapFailed,
                format!("failed to bind isolated WebRTC protocol bootstrap endpoint: {err}"),
            )
        })?;
    let bootstrap_stream =
        BootstrapStream::connect(&endpoint, remote_addr.clone(), BootstrapConfig::default())
            .await
            .map_err(|err| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::BootstrapFailed,
                    format!("failed to open WebRTC bootstrap path: {err}"),
                )
            })?;
    let channel = bootstrap_stream.open_channel().await.map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::BootstrapFailed,
            format!("failed to open WebRTC bootstrap signal channel: {err}"),
        )
    })?;
    let (mut sender, receiver) = channel.split();
    sender
        .send_signal(&start.bootstrap_signal)
        .await
        .map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::BootstrapFailed,
                format!("failed to send WebRTC bootstrap dial request: {err}"),
            )
        })?;

    let writer = spawn_bootstrap_signal_writer(sender);
    bootstrap
        .signal_senders
        .borrow_mut()
        .insert(session_key_string.clone(), writer);
    spawn_bootstrap_receiver_loop(
        core.clone(),
        rtc_control.clone(),
        bootstrap.clone(),
        receiver,
        None,
    );

    let (complete_tx, complete_rx) = oneshot::channel();
    bootstrap
        .pending_protocol_transport_prepares
        .borrow_mut()
        .insert(
            session_key_string.clone(),
            PendingProtocolTransportPrepare {
                sender: complete_tx,
                remote,
                transport_intent,
            },
        );
    let _cleanup =
        PendingProtocolTransportPrepareCleanup::new(bootstrap.clone(), session_key_string.clone());

    let mut start_value = to_protocol_value(start)?;
    dispatch_main_rtc_commands_from_result(
        core,
        rtc_control,
        bootstrap,
        post_message,
        &mut start_value,
    )
    .map_err(|error| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            js_error_message(&error),
        )
    })?;

    let result = complete_rx.await.map_err(|_| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::Closed,
            "WebRTC protocol transport preparation channel closed",
        )
    })?;
    endpoint.close().await;
    result
}

struct PendingProtocolTransportPrepareCleanup {
    bootstrap: Rc<WorkerBootstrapRuntime>,
    session_key: String,
}

impl PendingProtocolTransportPrepareCleanup {
    fn new(bootstrap: Rc<WorkerBootstrapRuntime>, session_key: String) -> Self {
        Self {
            bootstrap,
            session_key,
        }
    }
}

impl Drop for PendingProtocolTransportPrepareCleanup {
    fn drop(&mut self) {
        self.bootstrap
            .pending_protocol_transport_prepares
            .borrow_mut()
            .remove(&self.session_key);
        self.bootstrap
            .signal_senders
            .borrow_mut()
            .remove(&self.session_key);
    }
}

pub(super) fn complete_pending_protocol_transport_prepare_from_result(
    core: Rc<BrowserWorkerRuntimeCore>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
    result: &Value,
) -> Result<(), JsValue> {
    let Ok(payload) = serde_json::from_value::<PendingPrepareResultPayload>(result.clone()) else {
        return Ok(());
    };
    let Some(session_key) = payload.session_key.as_deref() else {
        return Ok(());
    };
    let Some(session) = payload.session else {
        return Ok(());
    };
    let action = pending_prepare_action_from_session(&session);
    let Some(action) = action else {
        return Ok(());
    };
    let pending = bootstrap
        .pending_protocol_transport_prepares
        .borrow_mut()
        .remove(session_key);
    let Some(pending) = pending else {
        return Ok(());
    };
    WorkerSessionKey::new(session_key.to_owned())
        .map_err(|err| wire_error_to_js(err.wire_error()))?;
    match action {
        PendingPrepareAction::WebRtc => {
            let Some(dial_id) = session.dial_id else {
                let _ = pending.sender.send(Err(BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    "prepared WebRTC session is missing dial id",
                )));
                return Ok(());
            };
            let result = core
                .open_node()
                .and_then(|node| node.add_protocol_transport_session_addr(pending.remote, dial_id));
            let _ = pending.sender.send(result);
        }
        PendingPrepareAction::Relay => {
            let _ = pending
                .sender
                .send(if pending.transport_intent.allows_iroh_relay_fallback() {
                    Ok(())
                } else {
                    Err(BrowserWorkerError::new(
                        BrowserWorkerErrorCode::WebRtcFailed,
                        "WebRTC-only protocol transport preparation resolved to relay",
                    ))
                });
        }
        PendingPrepareAction::Fail => {
            let _ = pending.sender.send(Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "WebRTC protocol transport preparation failed",
            )));
        }
        PendingPrepareAction::Closed => {
            let _ = pending.sender.send(Err(BrowserWorkerError::closed()));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingPrepareResultPayload {
    #[serde(default)]
    session_key: Option<String>,
    #[serde(default)]
    session: Option<PendingPrepareResultSession>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingPrepareResultSession {
    #[serde(default)]
    dial_id: Option<DialId>,
    #[serde(default)]
    resolved_transport: Option<String>,
    #[serde(default)]
    channel_attachment: Option<String>,
    #[serde(default)]
    lifecycle: Option<String>,
}

enum PendingPrepareAction {
    WebRtc,
    Relay,
    Fail,
    Closed,
}

fn pending_prepare_action_from_session(
    session: &PendingPrepareResultSession,
) -> Option<PendingPrepareAction> {
    match (
        session.resolved_transport.as_deref(),
        session.channel_attachment.as_deref(),
        session.lifecycle.as_deref(),
    ) {
        (_, Some("open"), _) => Some(PendingPrepareAction::WebRtc),
        (Some("irohRelay"), _, _) | (_, _, Some("relaySelected")) => {
            Some(PendingPrepareAction::Relay)
        }
        (_, _, Some("failed")) | (_, Some("failed"), _) => Some(PendingPrepareAction::Fail),
        (_, _, Some("closed")) | (_, Some("closed"), _) => Some(PendingPrepareAction::Closed),
        _ => None,
    }
}
