use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

use crate::core::bootstrap::{BootstrapConfig, BootstrapSignalSender, BootstrapStream};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

use super::dial::{PendingWorkerDial, complete_pending_dial_from_result};
use super::rtc_control::{WorkerRtcControlPort, dispatch_main_rtc_commands_from_result};
use super::wire::wire_error_to_js;
use super::*;

pub(super) struct WorkerBootstrapRuntime {
    pub(super) connection_tx: mpsc::UnboundedSender<Connection>,
    connection_rx: RefCell<Option<mpsc::UnboundedReceiver<Connection>>>,
    pub(super) signal_senders: RefCell<HashMap<String, BootstrapSignalWriter>>,
    pub(super) pending_dials: RefCell<HashMap<String, PendingWorkerDial>>,
    accept_loop_started: Cell<bool>,
}

#[derive(Clone)]
pub(super) struct BootstrapSignalWriter {
    tx: mpsc::UnboundedSender<BootstrapSignalBatch>,
}

struct BootstrapSignalBatch {
    signals: Vec<WebRtcSignal>,
}

impl WorkerBootstrapRuntime {
    pub(super) fn new() -> Self {
        let (connection_tx, connection_rx) = mpsc::unbounded_channel();
        Self {
            connection_tx,
            connection_rx: RefCell::new(Some(connection_rx)),
            signal_senders: RefCell::new(HashMap::new()),
            pending_dials: RefCell::new(HashMap::new()),
            accept_loop_started: Cell::new(false),
        }
    }
}

pub(super) fn start_bootstrap_accept_loop(
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
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
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
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
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
    receiver: crate::core::bootstrap::BootstrapSignalReceiver,
    alpn: Option<String>,
) {
    spawn_bootstrap_receiver_loop_with_sender(core, rtc_control, bootstrap, receiver, None, alpn);
}

fn spawn_bootstrap_receiver_loop_with_sender(
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
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
            let input = WorkerBootstrapSignalInput {
                signal,
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
            let mut result = match to_protocol_value(result) {
                Ok(result) => result,
                Err(_) => return,
            };
            let _ = send_outbound_signals_from_result(&bootstrap, &result);
            let _ = complete_pending_dial_from_result(core.clone(), bootstrap.clone(), &result);
            let _ = dispatch_main_rtc_commands_from_result(
                &core,
                &rtc_control,
                &bootstrap,
                None,
                &mut result,
            );
        }
    });
}

pub(super) fn send_outbound_signals_from_result(
    bootstrap: &Rc<WorkerBootstrapRuntime>,
    result: &Value,
) -> Result<(), JsValue> {
    let signals = outbound_signals_from_protocol_value(result)
        .map_err(|err| wire_error_to_js(err.wire_error()))?;
    if signals.is_empty() {
        return Ok(());
    }
    let mut by_session: HashMap<String, Vec<WebRtcSignal>> = HashMap::new();
    for signal in signals {
        by_session
            .entry(WorkerSessionKey::from(signal.dial_id()).as_str().to_owned())
            .or_default()
            .push(signal);
    }
    for (session_key, signals) in by_session {
        let sender = bootstrap.signal_senders.borrow().get(&session_key).cloned();
        let Some(sender) = sender else {
            let message =
                format!("missing bootstrap signal sender for WebRTC session {session_key}");
            return Err(wire_error_to_js(
                BrowserWorkerError::new(BrowserWorkerErrorCode::BootstrapFailed, message)
                    .wire_error(),
            ));
        };
        let _ = sender.tx.send(BootstrapSignalBatch { signals });
    }
    Ok(())
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
