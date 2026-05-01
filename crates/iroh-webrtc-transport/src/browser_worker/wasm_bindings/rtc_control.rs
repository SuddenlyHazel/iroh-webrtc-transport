use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::Duration,
};

use serde::Deserialize;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Event, MessageEvent, MessagePort, RtcDataChannel};

use super::data_channel::attach_data_channel_from_main_result;
use super::wire::{js_error, js_error_message, js_value_to_json, json_to_js, wire_error_to_js};
use super::*;

const MAIN_RTC_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const MAIN_RTC_CONTROL_PORT_MISSING: &str = "main-thread RTC control port is not attached";

pub(super) struct WorkerRtcControlPort {
    port: MessagePort,
    pending: Rc<RefCell<HashMap<u64, PendingMainRtcRequest>>>,
    next_request_id: Rc<Cell<u64>>,
    channel_handlers: Rc<RefCell<Vec<WorkerRtcDataChannelHandlers>>>,
    _message_handler: Closure<dyn FnMut(MessageEvent)>,
}

struct WorkerRtcDataChannelHandlers {
    _channel: RtcDataChannel,
    _open_handler: Closure<dyn FnMut(Event)>,
    _message_handler: Closure<dyn FnMut(MessageEvent)>,
    _error_handler: Closure<dyn FnMut(Event)>,
    _close_handler: Closure<dyn FnMut(Event)>,
}

pub(super) fn attach_rtc_control_port(
    core: &Rc<BrowserWorkerRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: &Rc<WorkerBootstrapRuntime>,
    post_message: Option<js_sys::Function>,
    port: MessagePort,
) -> Result<(), JsValue> {
    detach_rtc_control_port(rtc_control);

    let pending = Rc::new(RefCell::new(HashMap::new()));
    let next_request_id = Rc::new(Cell::new(0));
    let channel_handlers = Rc::new(RefCell::new(Vec::new()));
    let core = core.clone();
    let rtc_control_for_handler = rtc_control.clone();
    let bootstrap = bootstrap.clone();
    let post_message_for_handler = post_message.clone();
    let message_handler = Closure::wrap(Box::new(move |event: MessageEvent| {
        handle_rtc_control_response(
            core.clone(),
            rtc_control_for_handler.clone(),
            bootstrap.clone(),
            post_message_for_handler.clone(),
            event.data(),
        );
    }) as Box<dyn FnMut(_)>);

    port.set_onmessage(Some(message_handler.as_ref().unchecked_ref()));
    port.start();
    *rtc_control.borrow_mut() = Some(WorkerRtcControlPort {
        port,
        pending,
        next_request_id,
        channel_handlers,
        _message_handler: message_handler,
    });
    Ok(())
}

pub(super) fn detach_rtc_control_port(rtc_control: &Rc<RefCell<Option<WorkerRtcControlPort>>>) {
    if let Some(binding) = rtc_control.borrow_mut().take() {
        binding.port.set_onmessage(None);
        binding.port.close();
    }
}

pub(super) fn retain_data_channel_handlers(
    rtc_control: &Rc<RefCell<Option<WorkerRtcControlPort>>>,
    channel: RtcDataChannel,
    open_handler: Closure<dyn FnMut(Event)>,
    message_handler: Closure<dyn FnMut(MessageEvent)>,
    error_handler: Closure<dyn FnMut(Event)>,
    close_handler: Closure<dyn FnMut(Event)>,
) {
    let control = rtc_control.borrow();
    if let Some(binding) = control.as_ref() {
        binding
            .channel_handlers
            .borrow_mut()
            .push(WorkerRtcDataChannelHandlers {
                _channel: channel,
                _open_handler: open_handler,
                _message_handler: message_handler,
                _error_handler: error_handler,
                _close_handler: close_handler,
            });
    }
}

pub(super) fn dispatch_main_rtc_commands_from_result(
    core: &Rc<BrowserWorkerRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: &Rc<WorkerBootstrapRuntime>,
    post_message: Option<&js_sys::Function>,
    result: &mut Value,
) -> Result<(), JsValue> {
    let commands = main_rtc_commands_from_protocol_value(result)
        .map_err(|err| wire_error_to_js(err.wire_error()))?;
    if commands.is_empty() {
        return Ok(());
    }

    let mut control = rtc_control.borrow_mut();
    let Some(binding) = control.as_mut() else {
        return Err(JsValue::from_str(MAIN_RTC_CONTROL_PORT_MISSING));
    };

    for command in commands {
        let id = binding
            .next_request_id
            .get()
            .checked_add(1)
            .ok_or_else(|| JsValue::from_str("RTC control request id exhausted"))?;
        binding.next_request_id.set(id);
        let (envelope, pending) = main_rtc_command_request_envelope(id, &command)
            .map_err(|err| wire_error_to_js(err.wire_error()))?;
        let message = json_to_js(&envelope)?;
        binding.pending.borrow_mut().insert(id, pending);
        if let Err(error) = binding.port.post_message(&message) {
            binding.pending.borrow_mut().remove(&id);
            return Err(js_error("failed to post main RTC control request", error));
        }
        schedule_main_rtc_timeout(
            core.clone(),
            rtc_control.clone(),
            bootstrap.clone(),
            post_message.cloned(),
            id,
        );
    }
    clear_main_rtc_commands(result);
    Ok(())
}

#[cfg(test)]
pub(super) fn dispatch_main_rtc_commands_without_control_for_test(
    result: &mut Value,
) -> Result<(), JsValue> {
    let core = Rc::new(BrowserWorkerRuntimeCore::new());
    let rtc_control = Rc::new(RefCell::new(None));
    let bootstrap = Rc::new(WorkerBootstrapRuntime::new());
    dispatch_main_rtc_commands_from_result(&core, &rtc_control, &bootstrap, None, result)
}

fn schedule_main_rtc_timeout(
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
    post_message: Option<js_sys::Function>,
    id: u64,
) {
    spawn_local(async move {
        n0_future::time::sleep(MAIN_RTC_REQUEST_TIMEOUT).await;
        let pending = {
            let control = rtc_control.borrow();
            let Some(binding) = control.as_ref() else {
                return;
            };
            binding.pending.borrow_mut().remove(&id)
        };
        let Some(pending) = pending else {
            return;
        };
        let error = BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!(
                "main-thread WebRTC command {} timed out after {}ms",
                pending.command,
                MAIN_RTC_REQUEST_TIMEOUT.as_millis()
            ),
        )
        .wire_error();
        let Ok(payload) = worker_main_rtc_result_payload_from_response(&pending, None, Some(error))
        else {
            return;
        };
        let Ok(mut result) = core.handle_main_rtc_result_value(payload) else {
            return;
        };
        let _ = super::bootstrap::send_outbound_signals_from_result(&bootstrap, &result);
        let _ = super::dial::complete_pending_dial_from_result(
            core.clone(),
            bootstrap.clone(),
            &result,
        );
        let _ = dispatch_main_rtc_commands_from_result(
            &core,
            &rtc_control,
            &bootstrap,
            post_message.as_ref(),
            &mut result,
        );
    });
}

fn handle_rtc_control_response(
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
    post_message: Option<js_sys::Function>,
    message: JsValue,
) {
    if let Err(error) =
        apply_rtc_control_response(core, rtc_control, bootstrap, post_message.as_ref(), message)
    {
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_worker::rtc",
            error = %js_error_message(&error),
            "failed to process main-thread RTC response"
        );
    }
}

fn apply_rtc_control_response(
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
    post_message: Option<&js_sys::Function>,
    message: JsValue,
) -> Result<(), JsValue> {
    let response = decode_main_rtc_response(&message)?;
    let id = response.id;
    let pending = {
        let control = rtc_control.borrow();
        let Some(binding) = control.as_ref() else {
            return Ok(());
        };
        binding.pending.borrow_mut().remove(&id)
    };
    let Some(pending) = pending else {
        return Ok(());
    };

    let payload = if response.ok {
        let result = response.result.as_ref().map(|value| value.0.clone());
        attach_data_channel_from_main_result(
            &core,
            &rtc_control,
            &bootstrap,
            &pending,
            result.as_ref(),
        )?;
        let result_json = main_rtc_result_json(&pending, result.as_ref())?;
        worker_main_rtc_result_payload_from_response(&pending, Some(result_json), None)
            .map_err(|err| wire_error_to_js(err.wire_error()))?
    } else {
        let error = response_error(response.error)?;
        worker_main_rtc_result_payload_from_response(&pending, None, Some(error))
            .map_err(|err| wire_error_to_js(err.wire_error()))?
    };

    let mut result = core
        .handle_main_rtc_result_value(payload)
        .map_err(|err| wire_error_to_js(err.wire_error()))?;
    super::bootstrap::send_outbound_signals_from_result(&bootstrap, &result)?;
    super::dial::complete_pending_dial_from_result(core.clone(), bootstrap.clone(), &result)?;
    dispatch_main_rtc_commands_from_result(
        &core,
        &rtc_control,
        &bootstrap,
        post_message,
        &mut result,
    )?;
    Ok(())
}

fn main_rtc_result_json(
    pending: &PendingMainRtcRequest,
    result: Option<&JsValue>,
) -> Result<Value, JsValue> {
    if let Some(payload) = main_rtc_data_channel_result_payload(pending) {
        return Ok(payload);
    }
    match result {
        Some(result) => js_value_to_json(result.clone()),
        None => Ok(json!({})),
    }
}

fn decode_main_rtc_response(message: &JsValue) -> Result<MainRtcControlResponseEnvelope, JsValue> {
    let response: MainRtcControlResponseEnvelope = serde_wasm_bindgen::from_value(message.clone())
        .map_err(|err| {
            JsValue::from_str(&format!(
                "failed to decode RTC control response envelope: {err}"
            ))
        })?;
    if response.kind != "response" {
        return Err(JsValue::from_str(
            "RTC control message kind must be response",
        ));
    }
    Ok(response)
}

fn response_error(error: Option<PreservedJsValue>) -> Result<BrowserWorkerWireError, JsValue> {
    let Some(error) = error else {
        return Err(JsValue::from_str(
            "failed to read RTC control response error",
        ));
    };
    let value = js_value_to_json(error.0)?;
    serde_json::from_value(value)
        .map_err(|err| JsValue::from_str(&format!("failed to decode RTC control error: {err}")))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcControlResponseEnvelope {
    kind: String,
    id: u64,
    ok: bool,
    #[serde(default)]
    result: Option<PreservedJsValue>,
    #[serde(default)]
    error: Option<PreservedJsValue>,
}

#[derive(Debug, Deserialize)]
struct PreservedJsValue(#[serde(with = "serde_wasm_bindgen::preserve")] JsValue);
