use crate::{browser_protocol::MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND, error::Error};

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
use {
    crate::error::Result,
    serde::Deserialize,
    wasm_bindgen::{JsCast, JsValue},
};

pub(super) fn wire_error_code_for_command(command: Option<&str>, error: &Error) -> &'static str {
    match error {
        Error::BrowserWebRtcUnavailable => "unsupportedBrowser",
        Error::InvalidIceConfig(_) => "invalidStunUrl",
        Error::SessionClosed => "closed",
        Error::UnknownSession => "webrtcFailed",
        Error::WebRtc(_) if command == Some(MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND) => {
            "dataChannelFailed"
        }
        Error::InvalidConfig(_) if command == Some(MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND) => {
            "dataChannelFailed"
        }
        Error::InvalidAddr(_)
        | Error::InvalidFrame(_)
        | Error::PayloadTooLarge { .. }
        | Error::SendQueueFull
        | Error::WebRtc(_)
        | Error::InvalidConfig(_) => "webrtcFailed",
    }
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn error_to_wire_value_for_command(command: Option<&str>, error: Error) -> JsValue {
    error_to_wire_value_ref(command, &error)
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn error_to_wire_value_ref(command: Option<&str>, error: &Error) -> JsValue {
    let object = js_sys::Object::new();
    let _ = set_js_string(&object, "code", wire_error_code_for_command(command, error));
    let _ = set_js_string(&object, "message", &error.to_string());
    let detail = wire_error_detail(command, error);
    let _ = set_js_value(&object, "detail", detail);
    object.into()
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn wire_error_detail(command: Option<&str>, error: &Error) -> JsValue {
    let object = js_sys::Object::new();
    if let Some(command) = command {
        let _ = set_js_string(&object, "command", command);
    }
    let reason = match error {
        Error::UnknownSession => "unknownSessionKey",
        Error::SessionClosed => "sessionClosed",
        Error::BrowserWebRtcUnavailable => "unsupportedBrowser",
        Error::InvalidIceConfig(_) => "invalidStunUrl",
        Error::InvalidConfig(_) => "invalidConfig",
        Error::WebRtc(_) => "webRtcOperationFailed",
        Error::SendQueueFull => "sendQueueFull",
        Error::InvalidAddr(_) => "invalidAddr",
        Error::InvalidFrame(_) => "invalidFrame",
        Error::PayloadTooLarge { .. } => "payloadTooLarge",
    };
    let _ = set_js_string(&object, "reason", reason);
    object.into()
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn set_js_string(object: &js_sys::Object, key: &str, value: &str) -> Result<()> {
    set_js_value(object, key, JsValue::from_str(value))
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn set_js_value(object: &js_sys::Object, key: &str, value: JsValue) -> Result<()> {
    js_sys::Reflect::set(object, &JsValue::from_str(key), &value)
        .map(|_| ())
        .map_err(js_error)
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn js_array_buffer_like_to_vec(value: &JsValue) -> Result<Vec<u8>> {
    if value.is_instance_of::<js_sys::ArrayBuffer>() || js_sys::ArrayBuffer::is_view(value) {
        return Ok(js_sys::Uint8Array::new(value).to_vec());
    }
    if js_sys::Array::is_array(value) {
        let array = js_sys::Array::from(value);
        let mut bytes = Vec::with_capacity(array.length() as usize);
        for item in array.iter() {
            let Some(byte) = item.as_f64() else {
                return Err(Error::WebRtc("array-buffer byte is not a number".into()));
            };
            if !byte.is_finite() || byte.fract() != 0.0 || byte < 0.0 || byte > 255.0 {
                return Err(Error::WebRtc("array-buffer byte is out of range".into()));
            }
            bytes.push(byte as u8);
        }
        return Ok(bytes);
    }
    Err(Error::WebRtc(
        "value is not an ArrayBuffer, typed array, or byte array".into(),
    ))
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
fn js_error_property(value: &JsValue, key: &str) -> Option<String> {
    serde_wasm_bindgen::from_value::<JsErrorProperties>(value.clone())
        .ok()
        .and_then(|properties| match key {
            "name" => properties.name,
            "message" => properties.message,
            _ => None,
        })
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
#[derive(Deserialize)]
struct JsErrorProperties {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn js_error(value: JsValue) -> Error {
    Error::WebRtc(js_error_message(&value))
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn js_context_error(context: &str, error: JsValue) -> JsValue {
    JsValue::from_str(&format!("{context}: {}", js_error_message(&error)))
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn js_error_message(value: &JsValue) -> String {
    if let Some(message) = value.as_string() {
        return message;
    }
    if let Some(error) = value.dyn_ref::<js_sys::Error>() {
        let name = String::from(error.name());
        let message = String::from(error.message());
        return if name.is_empty() || name == "Error" {
            message
        } else if message.is_empty() {
            name
        } else {
            format!("{name}: {message}")
        };
    }
    let name = js_error_property(value, "name");
    let message = js_error_property(value, "message");
    match (name, message) {
        (Some(name), Some(message)) if !name.is_empty() && !message.is_empty() => {
            return format!("{name}: {message}");
        }
        (Some(name), _) if !name.is_empty() => return name,
        (_, Some(message)) if !message.is_empty() => return message,
        _ => {}
    }
    if let Ok(stringified) = js_sys::JSON::stringify(value) {
        if let Some(stringified) = stringified.as_string() {
            if !stringified.is_empty() && stringified != "null" && stringified != "{}" {
                return stringified;
            }
        }
    }
    "unknown JavaScript error".into()
}
