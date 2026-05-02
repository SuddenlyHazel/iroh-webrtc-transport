use std::{cell::RefCell, rc::Rc};

use serde::Serialize;
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::spawn_local;
use web_sys::{MessageEvent, MessagePort};

use super::WasmBrowserWorkerRuntime;
use super::wire::{
    js_context_error, js_error_message, js_number_property, js_optional_string_property,
    js_required_function_property,
};
use crate::browser_worker::BrowserWorkerProtocolRegistry;

thread_local! {
    static WORKER_HOST: RefCell<Option<BrowserWorkerHost>> = const { RefCell::new(None) };
}

struct BrowserWorkerHost {
    _runtime: Rc<WasmBrowserWorkerRuntime>,
    _post_message: Closure<dyn FnMut(JsValue, JsValue)>,
    _message_handler: Closure<dyn FnMut(MessageEvent)>,
}

#[wasm_bindgen]
pub fn start_browser_worker() -> Result<(), JsValue> {
    start_browser_worker_with_protocols(BrowserWorkerProtocolRegistry::default())
}

pub fn start_browser_worker_with_protocols(
    worker_protocols: BrowserWorkerProtocolRegistry,
) -> Result<(), JsValue> {
    WORKER_HOST.with(|host| {
        if host.borrow().is_some() {
            return Ok(());
        }

        let post_message = Closure::wrap(Box::new(move |message: JsValue, transfer: JsValue| {
            let _ = post_worker_message(&message, &transfer);
        }) as Box<dyn FnMut(_, _)>);

        let runtime = Rc::new(WasmBrowserWorkerRuntime::new(
            Some(
                post_message
                    .as_ref()
                    .unchecked_ref::<js_sys::Function>()
                    .clone(),
            ),
            worker_protocols,
        ));

        let handler_runtime = runtime.clone();
        let message_handler = Closure::wrap(Box::new(move |event: MessageEvent| {
            handle_worker_message(handler_runtime.clone(), event.data());
        }) as Box<dyn FnMut(_)>);

        add_worker_message_listener(&message_handler)?;
        *host.borrow_mut() = Some(BrowserWorkerHost {
            _runtime: runtime,
            _post_message: post_message,
            _message_handler: message_handler,
        });
        Ok(())
    })
}

fn handle_worker_message(runtime: Rc<WasmBrowserWorkerRuntime>, message: JsValue) {
    if let Ok(Some(port)) = rtc_control_port_from_message(&message) {
        if !runtime.is_rtc_control_port_attached() {
            if let Err(error) = runtime.attach_rtc_control_port(port) {
                post_error_response_for_request(&message, error);
            }
        } else {
            port.close();
        }
        if is_rtc_control_port_bootstrap_message(&message).unwrap_or(false) {
            return;
        }
    }

    spawn_local(async move {
        let command = js_optional_string_property(&message, "command")
            .ok()
            .flatten();
        match runtime.handle_message(message.clone()).await {
            Ok(_) => {
                if command.as_deref() == Some(super::WORKER_NODE_CLOSE_COMMAND) {
                    runtime.close();
                    close_worker();
                }
            }
            Err(error) => {
                if command.as_deref() == Some(super::WORKER_NODE_CLOSE_COMMAND) {
                    post_node_close_response(&message);
                    runtime.close();
                    close_worker();
                    return;
                }
                post_error_response_for_request(&message, error);
            }
        }
    });
}

fn add_worker_message_listener(
    message_handler: &Closure<dyn FnMut(MessageEvent)>,
) -> Result<(), JsValue> {
    let global = js_sys::global();
    let add_event_listener = js_required_function_property(
        &global,
        "addEventListener",
        "worker global addEventListener is not a function",
    )?;
    add_event_listener
        .call2(
            &global,
            &JsValue::from_str("message"),
            message_handler.as_ref(),
        )
        .map(|_| ())
        .map_err(|err| js_context_error("failed to install worker message listener", err))
}

fn post_worker_message(message: &JsValue, transfer: &JsValue) -> Result<(), JsValue> {
    let global = js_sys::global();
    let post_message = js_required_function_property(
        &global,
        "postMessage",
        "worker global postMessage is not a function",
    )?;
    post_message
        .call2(&global, message, transfer)
        .map(|_| ())
        .map_err(|err| js_context_error("failed to post worker message", err))
}

fn rtc_control_port_from_message(message: &JsValue) -> Result<Option<MessagePort>, JsValue> {
    if !is_rtc_control_port_bootstrap_message(message)? {
        return Ok(None);
    }
    js_sys::Reflect::get(message, &JsValue::from_str("port"))
        .map_err(|err| js_context_error("failed to read RTC control port", err))?
        .dyn_into::<MessagePort>()
        .map(Some)
        .map_err(|_| JsValue::from_str("RTC control bootstrap port must be a MessagePort"))
}

fn is_rtc_control_port_bootstrap_message(message: &JsValue) -> Result<bool, JsValue> {
    Ok(js_optional_string_property(message, "kind")?.as_deref() == Some("rtc-control-port"))
}

fn post_error_response_for_request(message: &JsValue, error: JsValue) {
    let Some(id) = js_number_property(message, "id") else {
        return;
    };
    let response = serde_wasm_bindgen::to_value(&WorkerHostErrorResponseEnvelope {
        kind: "response",
        id,
        ok: false,
        error: wire_error_from_js(error),
    });
    if let Ok(response) = response {
        let _ = post_worker_message(&response, &js_sys::Array::new().into());
    }
}

fn post_node_close_response(message: &JsValue) {
    let Some(id) = js_number_property(message, "id") else {
        return;
    };
    let response = serde_wasm_bindgen::to_value(&WorkerHostSuccessResponseEnvelope {
        kind: "response",
        id,
        ok: true,
        result: NodeCloseResult { closed: true },
    });
    if let Ok(response) = response {
        let _ = post_worker_message(&response, &js_sys::Array::new().into());
    }
}

fn wire_error_from_js(error: JsValue) -> JsValue {
    if js_optional_string_property(&error, "code")
        .ok()
        .flatten()
        .is_some()
        && js_optional_string_property(&error, "message")
            .ok()
            .flatten()
            .is_some()
    {
        return error;
    }

    serde_wasm_bindgen::to_value(&WorkerHostWireErrorFallback {
        code: "webrtcFailed",
        message: js_error_message(&error),
    })
    .unwrap_or_else(|_| JsValue::from_str("webrtcFailed"))
}

fn close_worker() {
    let global = js_sys::global();
    let Ok(close) =
        js_required_function_property(&global, "close", "worker global close is not a function")
    else {
        return;
    };
    let _ = close.call0(&global);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerHostSuccessResponseEnvelope {
    kind: &'static str,
    id: f64,
    ok: bool,
    result: NodeCloseResult,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeCloseResult {
    closed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerHostErrorResponseEnvelope {
    kind: &'static str,
    id: f64,
    ok: bool,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    error: JsValue,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerHostWireErrorFallback<'a> {
    code: &'a str,
    message: String,
}
