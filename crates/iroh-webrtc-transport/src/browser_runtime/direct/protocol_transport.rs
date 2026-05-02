use std::{cell::RefCell, rc::Rc};

use crate::core::bootstrap::{BootstrapConfig, BootstrapStream};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use super::bootstrap::{
    BrowserBootstrapRuntime, spawn_bootstrap_receiver_loop, spawn_bootstrap_signal_writer,
};
use super::dial::spawn_direct_dialer_rtc_setup;
use super::js_boundary::runtime_error_to_js;
use super::rtc_registry::BrowserRtcControl;
use super::*;

pub(super) struct PendingProtocolTransportPrepare {
    pub(super) sender: oneshot::Sender<BrowserRuntimeResult<()>>,
    pub(super) remote: EndpointId,
    pub(super) transport_intent: BootstrapTransportIntent,
}

pub(super) fn start_protocol_transport_prepare_loop(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    mut receiver: mpsc::Receiver<ProtocolTransportPrepareRequest>,
) {
    spawn_local(async move {
        while let Some(request) = receiver.recv().await {
            let core = core.clone();
            let rtc_control = rtc_control.clone();
            let bootstrap = bootstrap.clone();
            spawn_local(async move {
                prepare_protocol_transport(core, rtc_control, bootstrap, request).await;
            });
        }
    });
}

async fn prepare_protocol_transport(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    request: ProtocolTransportPrepareRequest,
) {
    let result = prepare_protocol_transport_inner(
        &core,
        &rtc_control,
        &bootstrap,
        request.remote,
        request.alpn,
        request.transport_intent,
    )
    .await;
    let _ = request.response.send(result);
}

async fn prepare_protocol_transport_inner(
    core: &Rc<BrowserRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    remote: EndpointId,
    alpn: String,
    transport_intent: BootstrapTransportIntent,
) -> BrowserRuntimeResult<()> {
    let node = core.open_node()?;
    let remote_addr = EndpointAddr::new(remote);
    let start = node.allocate_dial_start(remote, alpn, transport_intent)?;
    let session_key = BrowserSessionKey::new(start.session_key.clone())?;
    let session_key_string = session_key.as_str().to_owned();
    let endpoint = Endpoint::builder(iroh::endpoint::presets::N0)
        .bind()
        .await
        .map_err(|err| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::BootstrapFailed,
                format!("failed to bind isolated WebRTC protocol bootstrap endpoint: {err}"),
            )
        })?;
    let bootstrap_stream =
        BootstrapStream::connect(&endpoint, remote_addr.clone(), BootstrapConfig::default())
            .await
            .map_err(|err| {
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::BootstrapFailed,
                    format!("failed to open WebRTC bootstrap path: {err}"),
                )
            })?;
    let channel = bootstrap_stream.open_channel().await.map_err(|err| {
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
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::BootstrapFailed,
                format!("failed to send WebRTC bootstrap dial request: {err}"),
            )
        })?;

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

    spawn_direct_dialer_rtc_setup(
        core.clone(),
        rtc_control.clone(),
        bootstrap.clone(),
        writer,
        start,
    );

    let result = complete_rx.await.map_err(|_| {
        BrowserRuntimeError::new(
            BrowserRuntimeErrorCode::Closed,
            "WebRTC protocol transport preparation channel closed",
        )
    })?;
    endpoint.close().await;
    result
}

struct PendingProtocolTransportPrepareCleanup {
    bootstrap: Rc<BrowserBootstrapRuntime>,
    session_key: String,
}

impl PendingProtocolTransportPrepareCleanup {
    fn new(bootstrap: Rc<BrowserBootstrapRuntime>, session_key: String) -> Self {
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
    core: Rc<BrowserRuntimeCore>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    session_key: Option<&str>,
    session: Option<&BrowserSessionSnapshot>,
) -> Result<(), JsValue> {
    let Some(session_key) = session_key else {
        return Ok(());
    };
    let Some(session) = session else {
        return Ok(());
    };
    let action = pending_prepare_action_from_session(session);
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
    BrowserSessionKey::new(session_key.to_owned()).map_err(|err| runtime_error_to_js(err))?;
    match action {
        PendingPrepareAction::WebRtc => {
            let result = core.open_node().and_then(|node| {
                node.add_protocol_transport_session_addr(pending.remote, session.dial_id)
            });
            let _ = pending.sender.send(result);
        }
        PendingPrepareAction::Relay => {
            let _ = pending
                .sender
                .send(if pending.transport_intent.allows_iroh_relay_fallback() {
                    Ok(())
                } else {
                    Err(BrowserRuntimeError::new(
                        BrowserRuntimeErrorCode::WebRtcFailed,
                        "WebRTC-only protocol transport preparation resolved to relay",
                    ))
                });
        }
        PendingPrepareAction::Fail => {
            let _ = pending.sender.send(Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                "WebRTC protocol transport preparation failed",
            )));
        }
        PendingPrepareAction::Closed => {
            let _ = pending.sender.send(Err(BrowserRuntimeError::closed()));
        }
    }
    Ok(())
}

enum PendingPrepareAction {
    WebRtc,
    Relay,
    Fail,
    Closed,
}

fn pending_prepare_action_from_session(
    session: &BrowserSessionSnapshot,
) -> Option<PendingPrepareAction> {
    match (
        session.resolved_transport,
        session.channel_attachment,
        session.lifecycle,
    ) {
        (_, DataChannelAttachmentState::Open, _) => Some(PendingPrepareAction::WebRtc),
        (Some(BrowserResolvedTransport::IrohRelay), _, _)
        | (_, _, BrowserSessionLifecycle::RelaySelected) => Some(PendingPrepareAction::Relay),
        (_, _, BrowserSessionLifecycle::Failed) | (_, DataChannelAttachmentState::Failed, _) => {
            Some(PendingPrepareAction::Fail)
        }
        (_, _, BrowserSessionLifecycle::Closed) | (_, DataChannelAttachmentState::Closed, _) => {
            Some(PendingPrepareAction::Closed)
        }
        _ => None,
    }
}
