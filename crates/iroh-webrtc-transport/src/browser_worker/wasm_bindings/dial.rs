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

pub(super) struct PendingWorkerDial {
    sender: oneshot::Sender<BrowserWorkerResult<WorkerProtocolConnectionInfo>>,
    remote_addr: EndpointAddr,
    alpn: String,
}

struct PendingDialCleanup {
    bootstrap: Rc<WorkerBootstrapRuntime>,
    session_key: String,
}

impl PendingDialCleanup {
    fn new(bootstrap: Rc<WorkerBootstrapRuntime>, session_key: String) -> Self {
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

pub(super) async fn handle_dial_command_value(
    core: &Rc<BrowserWorkerRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: &Rc<WorkerBootstrapRuntime>,
    post_message: Option<&js_sys::Function>,
    payload: Value,
) -> BrowserWorkerResult<Value> {
    let node = core.open_node()?;
    let WorkerCommand::Dial {
        remote_addr,
        alpn,
        transport_intent,
    } = WorkerCommand::decode(WORKER_DIAL_COMMAND, payload.clone())?
    else {
        unreachable!("worker dial command decoded to another variant");
    };
    let remote = remote_addr.id;
    if !transport_intent.uses_webrtc() {
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::dial",
            remote = %remote,
            alpn = %alpn,
            transport_intent = ?transport_intent,
            "delegating non-WebRTC dial to core worker runtime"
        );
        return core
            .handle_command_value(WORKER_DIAL_COMMAND, payload)
            .await
            .map_err(|error| BrowserWorkerError::new(error.code, error.message));
    }

    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::dial",
        remote = %remote,
        alpn = %alpn,
        transport_intent = ?transport_intent,
        "starting WebRTC bootstrap dial"
    );
    let start = node.allocate_dial_start(remote_addr.id, alpn.clone(), transport_intent)?;
    let session_key = WorkerSessionKey::new(start.session_key.clone())?;
    let session_key_string = session_key.as_str().to_owned();
    let endpoint = node.relay_endpoint()?;
    let bootstrap_stream =
        BootstrapStream::connect(&endpoint, remote_addr.clone(), BootstrapConfig::default())
            .await
            .map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::dial",
                    session_key = %session_key.as_str(),
                    remote = %remote,
                    %err,
                    "failed to open WebRTC bootstrap path"
                );
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::BootstrapFailed,
                    format!("failed to open WebRTC bootstrap path: {err}"),
                )
            })?;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::dial",
        session_key = %session_key.as_str(),
        remote = %remote,
        "opened WebRTC bootstrap path"
    );
    let channel = bootstrap_stream.open_channel().await.map_err(|err| {
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_worker::dial",
            session_key = %session_key.as_str(),
            remote = %remote,
            %err,
            "failed to open WebRTC bootstrap signal channel"
        );
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
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::dial",
                session_key = %session_key.as_str(),
                remote = %remote,
                %err,
                "failed to send WebRTC bootstrap dial request"
            );
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::BootstrapFailed,
                format!("failed to send WebRTC bootstrap dial request: {err}"),
            )
        })?;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::dial",
        session_key = %session_key.as_str(),
        remote = %remote,
        "sent WebRTC bootstrap dial request"
    );

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
    bootstrap.pending_dials.borrow_mut().insert(
        session_key_string.clone(),
        PendingWorkerDial {
            sender: complete_tx,
            remote_addr,
            alpn,
        },
    );
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::dial",
        session_key = %session_key.as_str(),
        remote = %remote,
        "registered pending WebRTC dial completion"
    );
    let _pending_dial_cleanup = PendingDialCleanup::new(bootstrap.clone(), session_key_string);

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

    let connection = complete_rx.await.map_err(|_| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::Closed,
            "WebRTC dial completion channel closed",
        )
    })??;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::dial",
        session_key = %session_key.as_str(),
        remote = %remote,
        "completed WebRTC dial"
    );
    to_protocol_value(connection)
}

pub(super) fn complete_pending_dial_from_result(
    core: Rc<BrowserWorkerRuntimeCore>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
    result: &Value,
) -> Result<(), JsValue> {
    let Ok(payload) = serde_json::from_value::<PendingDialResultPayload>(result.clone()) else {
        return Ok(());
    };
    let Some(session_key) = payload.session_key.as_deref() else {
        return Ok(());
    };
    let Some(session) = payload.session else {
        return Ok(());
    };
    let session_value = serde_json::to_value(session).map_err(|err| {
        JsValue::from_str(&format!(
            "failed to encode pending dial session state: {err}"
        ))
    })?;
    let action = pending_dial_action_from_session(&session_value);
    let Some(action) = action else {
        return Ok(());
    };
    let pending = bootstrap.pending_dials.borrow_mut().remove(session_key);
    let Some(pending) = pending else {
        return Ok(());
    };
    let session_key = WorkerSessionKey::new(session_key.to_owned())
        .map_err(|err| wire_error_to_js(err.wire_error()))?;
    match action {
        PendingDialAction::WebRtc => {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::dial",
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
                target: "iroh_webrtc_transport::browser_worker::dial",
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
            let message = terminal_diagnostic_from_protocol_value(result)
                .unwrap_or_else(|| "WebRTC dial failed".to_owned());
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::dial",
                session_key = %session_key.as_str(),
                diagnostic = %message,
                "pending dial failed"
            );
            let _ = pending.sender.send(Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                message,
            )));
        }
        PendingDialAction::Closed => {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::dial",
                session_key = %session_key.as_str(),
                "pending dial closed"
            );
            let _ = pending.sender.send(Err(BrowserWorkerError::closed()));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingDialResultPayload {
    #[serde(default)]
    session_key: Option<String>,
    #[serde(default)]
    session: Option<PendingDialResultSession>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingDialResultSession {
    #[serde(default)]
    resolved_transport: Option<String>,
    #[serde(default)]
    channel_attachment: Option<String>,
    #[serde(default)]
    lifecycle: Option<String>,
}
