#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
use crate::error::Error;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
use {
    serde::Deserialize,
    wasm_bindgen::{JsCast, JsValue},
};

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
