use super::*;
use serde::Serialize;
use wasm_bindgen::{JsCast, JsValue};

pub(super) fn js_message_to_json(message: JsValue) -> Result<Value, JsValue> {
    js_value_to_json(message)
}

pub(super) fn worker_message_result_to_js(
    message: JsValue,
    transfer: js_sys::Array,
) -> Result<JsValue, JsValue> {
    if transfer.length() == 0 {
        return Ok(message);
    }
    let wrapper = js_sys::Object::new();
    js_sys::Reflect::set(&wrapper, &JsValue::from_str("message"), &message)
        .map_err(|err| js_error("failed to wrap worker response message", err))?;
    js_sys::Reflect::set(&wrapper, &JsValue::from_str("transfer"), transfer.as_ref())
        .map_err(|err| js_error("failed to attach worker response transfer list", err))?;
    Ok(wrapper.into())
}

pub(super) fn transfer_list_for_worker_response(
    command: &str,
    response: &JsValue,
) -> Result<js_sys::Array, JsValue> {
    let transfer = js_sys::Array::new();
    for requirement in crate::browser_protocol::response_transfer_requirements_for_command(command)
    {
        if requirement.kind != "array-buffer" {
            continue;
        }
        let value = js_value_at_path(response, requirement.path)?;
        if value.is_undefined() || value.is_null() {
            continue;
        }
        let Some(buffer) = transferable_array_buffer_for_js_value(&value)? else {
            continue;
        };
        set_js_value_at_path(response, requirement.path, buffer.as_ref())?;
        transfer.push(buffer.as_ref());
    }
    Ok(transfer)
}

pub(super) fn post_worker_message(
    post_message: &js_sys::Function,
    message: &JsValue,
    transfer: &js_sys::Array,
) -> Result<(), JsValue> {
    post_message
        .call2(&JsValue::UNDEFINED, message, transfer.as_ref())
        .map(|_| ())
        .map_err(|err| js_error("failed to post worker response", err))
}

pub(super) fn js_optional_string_property(
    value: &JsValue,
    key: &str,
) -> Result<Option<String>, JsValue> {
    let property = js_sys::Reflect::get(value, &JsValue::from_str(key))
        .map_err(|err| js_error("failed to read worker message property", err))?;
    if property.is_undefined() || property.is_null() {
        return Ok(None);
    }
    property.as_string().map(Some).ok_or_else(|| {
        JsValue::from_str(&format!("worker message property {key:?} must be a string"))
    })
}

pub(super) fn js_number_property(value: &JsValue, key: &str) -> Option<f64> {
    js_sys::Reflect::get(value, &JsValue::from_str(key))
        .ok()
        .and_then(|value| value.as_f64())
}

pub(super) fn js_required_function_property(
    value: &JsValue,
    key: &str,
    missing_message: &str,
) -> Result<js_sys::Function, JsValue> {
    js_sys::Reflect::get(value, &JsValue::from_str(key))
        .map_err(|err| js_context_error(&format!("failed to read JS property {key:?}"), err))?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| JsValue::from_str(missing_message))
}

fn js_value_at_path(root: &JsValue, path: &str) -> Result<JsValue, JsValue> {
    let mut value = root.clone();
    for segment in path.split('.') {
        value = js_sys::Reflect::get(&value, &JsValue::from_str(segment))
            .map_err(|err| js_error("failed to read worker protocol path", err))?;
        if value.is_undefined() || value.is_null() {
            return Ok(value);
        }
    }
    Ok(value)
}

fn set_js_value_at_path(root: &JsValue, path: &str, value: &JsValue) -> Result<(), JsValue> {
    let mut segments = path.split('.').peekable();
    let mut object = root.clone();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            js_sys::Reflect::set(&object, &JsValue::from_str(segment), value)
                .map_err(|err| js_error("failed to set worker protocol path", err))?;
            return Ok(());
        }
        object = js_sys::Reflect::get(&object, &JsValue::from_str(segment))
            .map_err(|err| js_error("failed to walk worker protocol path", err))?;
    }
    Ok(())
}

pub(super) fn js_array_buffer_like_to_vec(value: &JsValue) -> Result<Vec<u8>, JsValue> {
    if value.is_instance_of::<js_sys::ArrayBuffer>() || js_sys::ArrayBuffer::is_view(value) {
        return Ok(js_sys::Uint8Array::new(value).to_vec());
    }
    if js_sys::Array::is_array(value) {
        let array = js_sys::Array::from(value);
        let mut bytes = Vec::with_capacity(array.length() as usize);
        for item in array.iter() {
            let Some(byte) = item.as_f64() else {
                return Err(JsValue::from_str(
                    "array-buffer transfer byte is not a number",
                ));
            };
            if !byte.is_finite() || byte.fract() != 0.0 || byte < 0.0 || byte > 255.0 {
                return Err(JsValue::from_str(
                    "array-buffer transfer byte is out of range",
                ));
            }
            bytes.push(byte as u8);
        }
        return Ok(bytes);
    }
    Err(JsValue::from_str(
        "array-buffer transfer path is not an ArrayBuffer, typed array, or byte array",
    ))
}

fn transferable_array_buffer_for_js_value(
    value: &JsValue,
) -> Result<Option<js_sys::ArrayBuffer>, JsValue> {
    if value.is_instance_of::<js_sys::ArrayBuffer>() {
        return value
            .clone()
            .dyn_into::<js_sys::ArrayBuffer>()
            .map(Some)
            .map_err(|err| js_error("failed to read ArrayBuffer transfer", err));
    }
    if js_sys::ArrayBuffer::is_view(value) {
        let view = js_sys::Uint8Array::new(value);
        let copy = js_sys::Uint8Array::new_with_length(view.length());
        copy.set(&view, 0);
        return Ok(Some(copy.buffer()));
    }
    if js_sys::Array::is_array(value) {
        let bytes = js_array_buffer_like_to_vec(value)?;
        let array = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(&bytes);
        return Ok(Some(array.buffer()));
    }
    Ok(None)
}

pub(super) fn js_value_to_json(value: JsValue) -> Result<Value, JsValue> {
    if value.is_undefined() || value.is_null() {
        return Ok(json!({}));
    }
    serde_wasm_bindgen::from_value(value)
        .map_err(|err| JsValue::from_str(&format!("failed to decode worker payload: {err}")))
}

pub(super) fn json_to_js(value: &Value) -> Result<JsValue, JsValue> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|err| JsValue::from_str(&format!("failed to encode worker result: {err}")))
}

pub(super) fn wire_error_to_js(error: BrowserWorkerWireError) -> JsValue {
    match error.serialize(&serde_wasm_bindgen::Serializer::json_compatible()) {
        Ok(value) => value,
        Err(err) => JsValue::from_str(&format!("worker command failed: {err}")),
    }
}

pub(super) fn js_error(message: &str, error: JsValue) -> JsValue {
    let detail = js_sys::JSON::stringify(&error)
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "<non-json error>".to_owned());
    JsValue::from_str(&format!("{message}: {detail}"))
}

pub(super) fn js_error_message(error: &JsValue) -> String {
    js_sys::JSON::stringify(error)
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "<non-json error>".to_owned())
}

pub(super) fn js_context_error(context: &str, error: JsValue) -> JsValue {
    JsValue::from_str(&format!("{context}: {}", js_error_message(&error)))
}
