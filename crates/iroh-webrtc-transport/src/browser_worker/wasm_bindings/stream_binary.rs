use std::rc::Rc;

use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};

use super::wire::{js_array_buffer_like_to_vec, wire_error_to_js};
use super::*;

pub(super) async fn handle_stream_binary_command(
    core: &Rc<BrowserWorkerRuntimeCore>,
    command: &str,
    payload: &JsValue,
) -> Result<Option<JsValue>, JsValue> {
    match command {
        STREAM_SEND_CHUNK_COMMAND => {
            let input = stream_send_input_from_js(payload)?;
            let node = core.open_node().map_err(worker_error_to_js)?;
            let result = node
                .send_stream_chunk(input.stream_key, &input.chunk, input.end_stream)
                .await
                .map_err(worker_error_to_js)?;
            stream_send_result_to_js(&result).map(Some)
        }
        STREAM_RECEIVE_CHUNK_COMMAND => {
            let input = stream_receive_input_from_js(payload)?;
            let node = core.open_node().map_err(worker_error_to_js)?;
            let result = node
                .receive_stream_chunk(input.stream_key, input.max_bytes)
                .await
                .map_err(worker_error_to_js)?;
            stream_receive_result_to_js(&result).map(Some)
        }
        _ => Ok(None),
    }
}

pub(super) async fn handle_stream_binary_message(
    core: &Rc<BrowserWorkerRuntimeCore>,
    message: &JsValue,
    command: Option<&str>,
    post_message: Option<&js_sys::Function>,
) -> Result<Option<JsValue>, JsValue> {
    let Some(command) = command else {
        return Ok(None);
    };
    if command != STREAM_SEND_CHUNK_COMMAND && command != STREAM_RECEIVE_CHUNK_COMMAND {
        return Ok(None);
    }
    let request: StreamBinaryRequestEnvelope =
        decode_stream_payload("worker stream message envelope", message.clone())?;
    if request.kind != "request" || request.command != command {
        return Ok(None);
    }
    let id = request.id;
    let response = match handle_stream_binary_command(core, command, &request.payload).await {
        Ok(Some(result)) => stream_success_response_to_js(id, result)?,
        Ok(None) => return Ok(None),
        Err(error) => stream_error_response_to_js(id, error)?,
    };
    let transfer = transfer_list_for_stream_response(command, &response)?;
    if let Some(post_message) = post_message {
        post_worker_message(post_message, &response, &transfer)?;
        return Ok(Some(JsValue::UNDEFINED));
    }
    worker_message_result_to_js(response, transfer).map(Some)
}

struct StreamSendInput {
    stream_key: WorkerStreamKey,
    chunk: Vec<u8>,
    end_stream: bool,
}

struct StreamReceiveInput {
    stream_key: WorkerStreamKey,
    max_bytes: Option<usize>,
}

fn stream_send_input_from_js(payload: &JsValue) -> Result<StreamSendInput, JsValue> {
    let meta: StreamSendChunkPayloadMetadata =
        decode_stream_payload("stream.send-chunk payload", payload.clone())?;
    let stream_key = parse_stream_key(&meta.stream_key)?;
    let chunk = js_array_buffer_like_to_vec(&meta.chunk).map_err(|error| {
        worker_wire_error_to_js(
            BrowserWorkerErrorCode::DataChannelFailed,
            &format!("malformed stream.send-chunk payload chunk: {error:?}"),
        )
    })?;
    Ok(StreamSendInput {
        stream_key,
        chunk,
        end_stream: meta.end_stream.unwrap_or(false),
    })
}

fn stream_receive_input_from_js(payload: &JsValue) -> Result<StreamReceiveInput, JsValue> {
    let meta: StreamReceiveChunkPayloadMetadata =
        decode_stream_payload("stream.receive-chunk payload", payload.clone())?;
    Ok(StreamReceiveInput {
        stream_key: parse_stream_key(&meta.stream_key)?,
        max_bytes: meta.max_bytes,
    })
}

fn stream_send_result_to_js(result: &WorkerStreamSendResult) -> Result<JsValue, JsValue> {
    let backpressure = match result.backpressure {
        WorkerStreamBackpressureState::Ready => "ready",
        WorkerStreamBackpressureState::Blocked => "blocked",
    };
    serde_wasm_bindgen::to_value(&StreamSendResultMetadata {
        accepted_bytes: result.accepted_bytes,
        buffered_bytes: result.buffered_bytes,
        backpressure: backpressure.to_owned(),
    })
    .map_err(|err| {
        worker_wire_error_to_js(
            BrowserWorkerErrorCode::WebRtcFailed,
            &format!("failed to encode stream send result: {err}"),
        )
    })
}

fn stream_receive_result_to_js(result: &WorkerStreamReceiveResult) -> Result<JsValue, JsValue> {
    let value = serde_wasm_bindgen::to_value(&StreamReceiveResultMetadata { done: result.done })
        .map_err(|err| {
            worker_wire_error_to_js(
                BrowserWorkerErrorCode::WebRtcFailed,
                &format!("failed to encode stream receive result: {err}"),
            )
        })?;
    if let Some(chunk) = &result.chunk {
        let bytes = js_sys::Uint8Array::new_with_length(chunk.len() as u32);
        bytes.copy_from(chunk);
        let object = value
            .clone()
            .dyn_into::<js_sys::Object>()
            .map_err(|_| JsValue::from_str("stream receive result must be an object"))?;
        set_js_value(&object, "chunk", bytes.buffer().into())?;
        return Ok(object.into());
    }
    Ok(value)
}

fn stream_success_response_to_js(id: u64, result: JsValue) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(&StreamSuccessResponseEnvelope {
        kind: "response",
        id,
        ok: true,
        result,
    })
    .map_err(|err| {
        worker_wire_error_to_js(
            BrowserWorkerErrorCode::WebRtcFailed,
            &format!("failed to encode stream success response: {err}"),
        )
    })
}

fn stream_error_response_to_js(id: u64, error: JsValue) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(&StreamErrorResponseEnvelope {
        kind: "response",
        id,
        ok: false,
        error,
    })
    .map_err(|err| {
        worker_wire_error_to_js(
            BrowserWorkerErrorCode::WebRtcFailed,
            &format!("failed to encode stream error response: {err}"),
        )
    })
}

fn transfer_list_for_stream_response(
    command: &str,
    response: &JsValue,
) -> Result<js_sys::Array, JsValue> {
    if command == STREAM_RECEIVE_CHUNK_COMMAND {
        return transfer_list_for_worker_response(command, response);
    }
    Ok(js_sys::Array::new())
}

fn parse_stream_key(value: &str) -> Result<WorkerStreamKey, JsValue> {
    let Some(value) = value.strip_prefix("stream-") else {
        return Err(worker_wire_error_to_js(
            BrowserWorkerErrorCode::WebRtcFailed,
            "payload field \"streamKey\" must start with stream-",
        ));
    };
    let stream_id = value.parse::<u64>().map_err(|err| {
        worker_wire_error_to_js(
            BrowserWorkerErrorCode::WebRtcFailed,
            &format!("payload field \"streamKey\" is not a valid stream id: {err}"),
        )
    })?;
    Ok(WorkerStreamKey(stream_id))
}

fn set_js_value(object: &js_sys::Object, key: &str, value: JsValue) -> Result<(), JsValue> {
    js_sys::Reflect::set(object, &JsValue::from_str(key), &value)
        .map(|_| ())
        .map_err(|err| {
            worker_wire_error_to_js(
                BrowserWorkerErrorCode::WebRtcFailed,
                &format!("failed to set stream result field {key}: {err:?}"),
            )
        })
}

fn worker_error_to_js(error: BrowserWorkerError) -> JsValue {
    wire_error_to_js(error.wire_error())
}

fn worker_wire_error_to_js(code: BrowserWorkerErrorCode, message: &str) -> JsValue {
    wire_error_to_js(BrowserWorkerError::new(code, message).wire_error())
}

fn decode_stream_payload<T: for<'de> Deserialize<'de>>(
    context: &str,
    value: JsValue,
) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(value).map_err(|err| {
        worker_wire_error_to_js(
            BrowserWorkerErrorCode::WebRtcFailed,
            &format!("malformed {context}: {err}"),
        )
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamBinaryRequestEnvelope {
    kind: String,
    id: u64,
    command: String,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    payload: JsValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamSendChunkPayloadMetadata {
    stream_key: String,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    chunk: JsValue,
    #[serde(default)]
    end_stream: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamReceiveChunkPayloadMetadata {
    stream_key: String,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamSendResultMetadata {
    accepted_bytes: usize,
    buffered_bytes: usize,
    backpressure: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamReceiveResultMetadata {
    done: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamSuccessResponseEnvelope {
    kind: &'static str,
    id: u64,
    ok: bool,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    result: JsValue,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamErrorResponseEnvelope {
    kind: &'static str,
    id: u64,
    ok: bool,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    error: JsValue,
}
