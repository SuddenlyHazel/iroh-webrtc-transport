use super::{capabilities::validate_data_channel_transferable, js_wire::*, *};
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

pub(super) struct MainRtcControlRequest {
    pub(super) id: u64,
    pub(super) command: String,
    pub(super) payload: JsValue,
}

pub(super) fn rtc_control_request_from_js(message: &JsValue) -> Result<MainRtcControlRequest> {
    let envelope: MainRtcControlRequestEnvelope =
        serde_wasm_bindgen::from_value(message.clone())
            .map_err(|err| Error::WebRtc(format!("malformed RTC control message: {err}")))?;
    if envelope.kind != "request" {
        return Err(Error::WebRtc(
            "malformed RTC control message: kind must be request".into(),
        ));
    }
    let command = envelope.command;
    if !command.starts_with("main.") {
        return Err(Error::WebRtc(format!(
            "unsupported RTC control command {command:?}"
        )));
    }
    let payload = envelope
        .payload
        .map(|value| value.0)
        .unwrap_or_else(|| js_sys::Object::new().into());
    if !payload.is_object() || payload.is_null() {
        return Err(Error::WebRtc(format!(
            "malformed payload for {command}: payload must be an object"
        )));
    }
    Ok(MainRtcControlRequest {
        id: envelope.id,
        command,
        payload,
    })
}

pub(super) fn rtc_control_message_id(message: &JsValue) -> Option<u64> {
    serde_wasm_bindgen::from_value::<MainRtcControlMessageId>(message.clone())
        .map(|message| message.id)
        .ok()
}

pub(super) fn rtc_control_success_response(id: u64, result: JsValue) -> Result<JsValue> {
    serde_wasm_bindgen::to_value(&MainRtcControlSuccessResponse {
        kind: "response",
        id,
        ok: true,
        result,
    })
    .map_err(|err| Error::WebRtc(format!("failed to encode RTC control response: {err}")))
}

pub(super) fn rtc_control_error_response(id: u64, error: JsValue) -> JsValue {
    serde_wasm_bindgen::to_value(&MainRtcControlErrorResponse {
        kind: "response",
        id,
        ok: false,
        error,
    })
    .unwrap_or_else(|_| JsValue::from_str("failed to encode RTC control error response"))
}

pub(super) fn transfer_list_for_main_rtc_result(result: &JsValue) -> Result<js_sys::Array> {
    let transfer = js_sys::Array::new();
    if !result.is_object() || result.is_null() {
        return Ok(transfer);
    }
    let channel = js_sys::Reflect::get(result, &JsValue::from_str("channel")).map_err(js_error)?;
    if channel.is_undefined() || channel.is_null() {
        return Ok(transfer);
    }
    let channel: RtcDataChannel = channel.dyn_into().map_err(js_error)?;
    validate_data_channel_transferable(&channel)?;
    transfer.push(channel.as_ref());
    Ok(transfer)
}

#[derive(Debug, Deserialize)]
struct MainRtcControlMessageId {
    id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MainRtcControlRequestEnvelope {
    kind: String,
    id: u64,
    command: String,
    #[serde(default)]
    payload: Option<PreservedJsValue>,
}

#[derive(Debug, Deserialize)]
struct PreservedJsValue(#[serde(with = "serde_wasm_bindgen::preserve")] JsValue);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcControlSuccessResponse {
    kind: &'static str,
    id: u64,
    ok: bool,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    result: JsValue,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MainRtcControlErrorResponse {
    kind: &'static str,
    id: u64,
    ok: bool,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    error: JsValue,
}

pub(super) fn post_rtc_control_response(
    port: &MessagePort,
    response: JsValue,
    transfer: Option<&js_sys::Array>,
) -> Result<()> {
    match transfer.filter(|transfer| transfer.length() > 0) {
        Some(transfer) => port
            .post_message_with_transferable(&response, &JsValue::from(transfer.clone()))
            .map_err(js_error),
        None => port.post_message(&response).map_err(js_error),
    }
}
