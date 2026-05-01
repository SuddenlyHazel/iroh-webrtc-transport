use super::*;
use std::{cell::RefCell, rc::Rc};

use serde::Deserialize;
use wasm_bindgen::JsValue;
use web_sys::MessagePort;

mod benchmark;
mod bootstrap;
mod data_channel;
mod dial;
mod rtc_control;
mod stream_binary;
mod wire;
mod worker_host;

use benchmark::{
    handle_benchmark_latency_command_value, handle_benchmark_throughput_command_value,
};
use bootstrap::{
    WorkerBootstrapRuntime, send_outbound_signals_from_result, start_bootstrap_accept_loop,
};
use dial::handle_dial_command_value;
use rtc_control::{
    WorkerRtcControlPort, attach_rtc_control_port as attach_worker_rtc_control_port,
    detach_rtc_control_port, dispatch_main_rtc_commands_from_result,
};
use stream_binary::handle_stream_binary_message;
use wire::{
    js_message_to_json, js_optional_string_property, json_to_js, post_worker_message,
    transfer_list_for_worker_response, wire_error_to_js, worker_message_result_to_js,
};

struct WasmBrowserWorkerRuntime {
    core: Rc<BrowserWorkerRuntimeCore>,
    rtc_control: Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: Rc<WorkerBootstrapRuntime>,
    post_message: Option<js_sys::Function>,
}

impl WasmBrowserWorkerRuntime {
    fn new(post_message: Option<js_sys::Function>) -> Self {
        crate::browser_console_tracing::install_worker_console_tracing();
        Self {
            core: Rc::new(BrowserWorkerRuntimeCore::new()),
            rtc_control: Rc::new(RefCell::new(None)),
            bootstrap: Rc::new(WorkerBootstrapRuntime::new()),
            post_message,
        }
    }

    fn attach_rtc_control_port(&self, port: MessagePort) -> Result<(), JsValue> {
        attach_worker_rtc_control_port(
            &self.core,
            &self.rtc_control,
            &self.bootstrap,
            self.post_message.clone(),
            port,
        )
    }

    fn detach_rtc_control_port(&self) {
        detach_rtc_control_port(&self.rtc_control);
    }

    fn is_rtc_control_port_attached(&self) -> bool {
        self.rtc_control.borrow().is_some()
    }

    fn close(&self) {
        self.detach_rtc_control_port();
    }

    async fn handle_message(&self, message: JsValue) -> Result<JsValue, JsValue> {
        let command = js_optional_string_property(&message, "command")?;
        if let Some(command) = command.as_deref() {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::command",
                command,
                "received browser worker command"
            );
        }
        if let Some(response) = handle_stream_binary_message(
            &self.core,
            &message,
            command.as_deref(),
            self.post_message.as_ref(),
        )
        .await?
        {
            return Ok(response);
        }
        let message_json = js_message_to_json(message)?;
        let response =
            match worker_response_from_message(message_json, |command, payload| async move {
                self.handle_command_value(&command, payload).await
            })
            .await
            {
                Ok(Some(mut response)) => {
                    if let Some(result) = successful_worker_response_result_mut(&mut response)? {
                        send_outbound_signals_from_result(&self.bootstrap, result)?;
                        dispatch_main_rtc_commands_from_result(
                            &self.core,
                            &self.rtc_control,
                            &self.bootstrap,
                            self.post_message.as_ref(),
                            result,
                        )?;
                    }
                    response
                }
                Ok(None) => return Ok(JsValue::UNDEFINED),
                Err(error) => return Err(wire_error_to_js(error.wire_error())),
            };
        if let Some(command) = command.as_deref() {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::command",
                command,
                "finished browser worker command"
            );
        }

        let response = json_to_js(&response)?;
        let transfer = command
            .as_deref()
            .map(|command| transfer_list_for_worker_response(command, &response))
            .transpose()?
            .unwrap_or_else(js_sys::Array::new);
        if let Some(post_message) = &self.post_message {
            post_worker_message(post_message, &response, &transfer)?;
            return Ok(JsValue::UNDEFINED);
        }
        worker_message_result_to_js(response, transfer)
    }

    async fn handle_command_value(
        &self,
        command: &str,
        payload: Value,
    ) -> Result<Value, BrowserWorkerWireError> {
        match command {
            WORKER_SPAWN_COMMAND => {
                start_bootstrap_accept_loop(
                    self.core.clone(),
                    self.rtc_control.clone(),
                    self.bootstrap.clone(),
                );
                self.core
                    .spawn_node_from_payload(payload, Some(self.bootstrap.connection_tx.clone()))
                    .await
                    .and_then(to_protocol_value)
                    .map_err(|err| err.wire_error())
            }
            WORKER_DIAL_COMMAND => handle_dial_command_value(
                &self.core,
                &self.rtc_control,
                &self.bootstrap,
                self.post_message.as_ref(),
                payload,
            )
            .await
            .map_err(|err| err.wire_error()),
            BENCHMARK_LATENCY_COMMAND => handle_benchmark_latency_command_value(
                &self.core,
                &self.rtc_control,
                &self.bootstrap,
                self.post_message.as_ref(),
                payload,
            )
            .await
            .map_err(|err| err.wire_error()),
            BENCHMARK_THROUGHPUT_COMMAND => handle_benchmark_throughput_command_value(
                &self.core,
                &self.rtc_control,
                &self.bootstrap,
                self.post_message.as_ref(),
                payload,
            )
            .await
            .map_err(|err| err.wire_error()),
            _ => self.core.handle_command_value(command, payload).await,
        }
    }
}

#[cfg(test)]
pub(super) fn dispatch_main_rtc_commands_without_control_for_test(
    result: &mut Value,
) -> Result<(), JsValue> {
    rtc_control::dispatch_main_rtc_commands_without_control_for_test(result)
}

pub use worker_host::start_browser_worker;

fn successful_worker_response_result_mut(
    response: &mut Value,
) -> Result<Option<&mut Value>, JsValue> {
    let envelope: WorkerEnvelopeStatus = serde_json::from_value(response.clone())
        .map_err(|err| JsValue::from_str(&format!("malformed worker response envelope: {err}")))?;
    if envelope.ok {
        Ok(response.get_mut("result"))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerEnvelopeStatus {
    ok: bool,
}
