use std::collections::HashSet;

use crate::error::{Error, Result};
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
use serde::{Deserialize, Serialize};

#[derive(Debug, Default)]
pub(super) struct BrowserWorkerRequestTracker {
    next_request_id: u64,
    pending: HashSet<u64>,
}

impl BrowserWorkerRequestTracker {
    #[cfg(test)]
    pub(super) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub(super) fn allocate(&mut self) -> Result<u64> {
        let id = self
            .next_request_id
            .checked_add(1)
            .ok_or_else(|| Error::WebRtc("worker request id exhausted".into()))?;
        self.next_request_id = id;
        if !self.pending.insert(id) {
            return Err(Error::WebRtc("duplicate worker request id".into()));
        }
        Ok(id)
    }

    pub(super) fn complete(&mut self, id: u64) -> bool {
        self.pending.remove(&id)
    }

    pub(super) fn fail_all(&mut self) -> Vec<u64> {
        let mut ids: Vec<_> = self.pending.iter().copied().collect();
        ids.sort_unstable();
        self.pending.clear();
        ids
    }
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
use {
    super::{
        capabilities::validate_data_channel_transferable, js_wire::*,
        rtc_control_wire::rtc_control_error_response, *,
    },
    std::{cell::RefCell, collections::HashMap, rc::Rc},
};

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) struct PendingWorkerRequest {
    pub(super) resolve: js_sys::Function,
    pub(super) reject: js_sys::Function,
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn create_message_channel_ports()
-> std::result::Result<(MessagePort, MessagePort), JsValue> {
    let constructor = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("MessageChannel"))
        .map_err(|err| js_bridge_error("failed to inspect MessageChannel constructor", err))?;
    if !constructor.is_function() {
        return Err(worker_bridge_wire_error(
            "unsupportedBrowser",
            "MessageChannel is unavailable in this runtime",
            None,
        ));
    }
    let channel = js_sys::Reflect::construct(
        constructor.unchecked_ref::<js_sys::Function>(),
        &js_sys::Array::new(),
    )
    .map_err(|err| js_bridge_error("failed to create MessageChannel", err))?;
    let port1 = js_sys::Reflect::get(&channel, &JsValue::from_str("port1"))
        .map_err(|err| js_bridge_error("failed to read MessageChannel.port1", err))?
        .dyn_into::<MessagePort>()
        .map_err(|err| js_bridge_error("MessageChannel.port1 is not a MessagePort", err))?;
    let port2 = js_sys::Reflect::get(&channel, &JsValue::from_str("port2"))
        .map_err(|err| js_bridge_error("failed to read MessageChannel.port2", err))?
        .dyn_into::<MessagePort>()
        .map_err(|err| js_bridge_error("MessageChannel.port2 is not a MessagePort", err))?;
    Ok((port1, port2))
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn worker_request_payload(
    payload: JsValue,
    command: &str,
) -> std::result::Result<JsValue, JsValue> {
    if payload.is_undefined() || payload.is_null() {
        return Ok(js_sys::Object::new().into());
    }
    if !payload.is_object() {
        return Err(worker_bridge_wire_error(
            "webrtcFailed",
            &format!("malformed payload for {command}: payload must be an object"),
            Some(command),
        ));
    }
    let object = js_sys::Object::new();
    js_sys::Object::assign(&object, payload.unchecked_ref::<js_sys::Object>());
    Ok(object.into())
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn worker_request_message(
    id: u64,
    command: &str,
    payload: JsValue,
) -> std::result::Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(&WorkerBridgeRequestEnvelope {
        kind: "request",
        id,
        command,
        payload,
    })
    .map_err(|err| {
        worker_bridge_wire_error(
            "webrtcFailed",
            &format!("failed to encode worker request envelope: {err}"),
            Some(command),
        )
    })
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn post_worker_message(
    worker: &Worker,
    message: &JsValue,
    transfer: Option<&js_sys::Array>,
) -> std::result::Result<(), JsValue> {
    match transfer.filter(|transfer| transfer.length() > 0) {
        Some(transfer) => worker
            .post_message_with_transfer(message, &JsValue::from(transfer.clone()))
            .map_err(|err| js_bridge_error("failed to post message to browser WebRTC worker", err)),
        None => worker
            .post_message(message)
            .map_err(|err| js_bridge_error("failed to post message to browser WebRTC worker", err)),
    }
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn transfer_list_for_worker_request(
    command: &str,
    payload: &JsValue,
) -> std::result::Result<Option<js_sys::Array>, JsValue> {
    let transfer = js_sys::Array::new();
    if !payload.is_object() || payload.is_null() {
        return Ok(None);
    }
    match command {
        WORKER_ATTACH_DATA_CHANNEL_COMMAND => {
            let channel = js_sys::Reflect::get(payload, &JsValue::from_str("channel"))
                .map_err(|err| js_bridge_error("failed to read worker request channel", err))?;
            if channel.is_undefined() || channel.is_null() {
                return Ok(None);
            }
            let channel = channel.dyn_into::<RtcDataChannel>().map_err(|err| {
                js_bridge_error("worker request channel is not an RTCDataChannel", err)
            })?;
            validate_data_channel_transferable(&channel)
                .map_err(|error| worker_bridge_error_from_error(Some(command), error))?;
            transfer.push(channel.as_ref());
        }
        STREAM_SEND_CHUNK_COMMAND => {
            let chunk = js_sys::Reflect::get(payload, &JsValue::from_str("chunk"))
                .map_err(|err| js_bridge_error("failed to read worker request chunk", err))?;
            if chunk.is_undefined() || chunk.is_null() {
                return Ok(None);
            }
            if !chunk.is_instance_of::<js_sys::ArrayBuffer>() {
                return Err(worker_bridge_wire_error(
                    "dataChannelFailed",
                    "stream.send-chunk payload chunk must be an ArrayBuffer",
                    Some(command),
                ));
            }
            transfer.push(&chunk);
        }
        _ => {}
    }
    if transfer.length() == 0 {
        Ok(None)
    } else {
        Ok(Some(transfer))
    }
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn handle_worker_message(
    worker: Worker,
    pending: Rc<RefCell<HashMap<u64, PendingWorkerRequest>>>,
    tracker: Rc<RefCell<BrowserWorkerRequestTracker>>,
    message: JsValue,
) {
    if let Some(kind) = worker_message_kind(&message) {
        match kind.as_str() {
            "response" => {
                resolve_worker_response(pending, tracker, &message);
            }
            "request" => {
                reject_worker_to_main_request(&worker, &message);
            }
            _ => {}
        }
        return;
    }
    if worker_response_id(&message).is_some() {
        resolve_worker_response(pending, tracker, &message);
    }
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn resolve_worker_response(
    pending: Rc<RefCell<HashMap<u64, PendingWorkerRequest>>>,
    tracker: Rc<RefCell<BrowserWorkerRequestTracker>>,
    message: &JsValue,
) {
    let Some(id) = worker_response_id(message) else {
        return;
    };
    let pending_request = {
        let mut pending = pending.borrow_mut();
        pending.remove(&id)
    };
    let Some(pending_request) = pending_request else {
        return;
    };
    {
        tracker.borrow_mut().complete(id);
    }
    if worker_response_ok(message) {
        let result = js_sys::Reflect::get(message, &JsValue::from_str("result"))
            .unwrap_or(JsValue::UNDEFINED);
        let _ = pending_request.resolve.call1(&JsValue::UNDEFINED, &result);
    } else {
        let error =
            js_sys::Reflect::get(message, &JsValue::from_str("error")).unwrap_or_else(|_| {
                worker_bridge_wire_error("webrtcFailed", "worker request failed", None)
            });
        let _ = pending_request.reject.call1(&JsValue::UNDEFINED, &error);
    }
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn reject_all_pending_worker_requests(
    pending: &Rc<RefCell<HashMap<u64, PendingWorkerRequest>>>,
    tracker: &Rc<RefCell<BrowserWorkerRequestTracker>>,
    error: JsValue,
) {
    let ids = {
        let mut tracker = tracker.borrow_mut();
        tracker.fail_all()
    };
    let pending_requests = {
        let mut pending = pending.borrow_mut();
        let mut pending_requests = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(pending_request) = pending.remove(&id) {
                pending_requests.push(pending_request);
            }
        }
        pending.clear();
        pending_requests
    };
    for pending_request in pending_requests {
        let _ = pending_request.reject.call1(&JsValue::UNDEFINED, &error);
    }
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn reject_worker_to_main_request(worker: &Worker, message: &JsValue) {
    let Some(id) = worker_response_id(message) else {
        return;
    };
    let command = worker_message_command(message);
    let response = rtc_control_error_response(
        id,
        worker_bridge_wire_error(
            "webrtcFailed",
            "worker-to-main RTC requests must use the Wasm RTC control port",
            command.as_deref(),
        ),
    );
    let _ = post_worker_message(worker, &response, None);
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn worker_response_id(message: &JsValue) -> Option<u64> {
    serde_wasm_bindgen::from_value::<WorkerBridgeResponseMetadata>(message.clone())
        .map(|response| response.id)
        .ok()
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn worker_response_ok(message: &JsValue) -> bool {
    serde_wasm_bindgen::from_value::<WorkerBridgeResponseMetadata>(message.clone())
        .map(|response| response.ok)
        .unwrap_or_else(|_| {
            js_sys::Reflect::get(message, &JsValue::from_str("ok"))
                .map(|ok| ok.as_bool().unwrap_or(false))
                .unwrap_or(false)
        })
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
fn worker_message_kind(message: &JsValue) -> Option<String> {
    serde_wasm_bindgen::from_value::<WorkerBridgeMessageKind>(message.clone())
        .ok()
        .and_then(|message| message.kind)
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
fn worker_message_command(message: &JsValue) -> Option<String> {
    serde_wasm_bindgen::from_value::<WorkerBridgeMessageCommand>(message.clone())
        .ok()
        .and_then(|message| message.command)
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn worker_error_message(event: &JsValue, fallback: &str) -> String {
    js_sys::Reflect::get(event, &JsValue::from_str("message"))
        .ok()
        .and_then(|value| value.as_string())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn js_bridge_error(message: &str, value: JsValue) -> JsValue {
    worker_bridge_wire_error(
        "webrtcFailed",
        &format!("{message}: {}", js_error_message(&value)),
        None,
    )
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn worker_bridge_error_from_error(command: Option<&str>, error: Error) -> JsValue {
    let code = wire_error_code_for_command(command, &error);
    worker_bridge_wire_error(code, &error.to_string(), command)
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn worker_bridge_wire_error(
    code: &str,
    message: &str,
    command: Option<&str>,
) -> JsValue {
    serde_wasm_bindgen::to_value(&WorkerBridgeWireErrorEnvelope {
        code: code.to_owned(),
        message: message.to_owned(),
        detail: WorkerBridgeErrorDetail {
            command: command.map(ToOwned::to_owned),
        },
    })
    .unwrap_or_else(|_| JsValue::from_str("failed to encode worker bridge error"))
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerBridgeRequestEnvelope<'a> {
    kind: &'static str,
    id: u64,
    command: &'a str,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    payload: JsValue,
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerBridgeResponseMetadata {
    #[serde(default)]
    _kind: Option<String>,
    id: u64,
    ok: bool,
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerBridgeMessageKind {
    #[serde(default)]
    kind: Option<String>,
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerBridgeMessageCommand {
    #[serde(default)]
    command: Option<String>,
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerBridgeWireErrorEnvelope {
    code: String,
    message: String,
    detail: WorkerBridgeErrorDetail,
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerBridgeErrorDetail {
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
}
