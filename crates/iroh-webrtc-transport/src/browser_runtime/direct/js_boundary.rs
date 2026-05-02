use wasm_bindgen::{JsCast, JsValue};

use super::*;

pub(super) fn js_array_buffer_like_to_vec(value: &JsValue) -> Result<Vec<u8>, JsValue> {
    if let Some(buffer) = value.dyn_ref::<js_sys::ArrayBuffer>() {
        return Ok(js_sys::Uint8Array::new(buffer).to_vec());
    }
    if let Some(array) = value.dyn_ref::<js_sys::Uint8Array>() {
        return Ok(array.to_vec());
    }
    Err(JsValue::from_str("expected ArrayBuffer or Uint8Array"))
}

pub(super) fn js_error_message(error: &JsValue) -> String {
    if let Some(message) = js_sys::Reflect::get(error, &JsValue::from_str("message"))
        .ok()
        .and_then(|value| value.as_string())
    {
        return message;
    }
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}

pub(super) fn runtime_error_to_js(error: BrowserRuntimeError) -> JsValue {
    JsValue::from_str(&error.message)
}
