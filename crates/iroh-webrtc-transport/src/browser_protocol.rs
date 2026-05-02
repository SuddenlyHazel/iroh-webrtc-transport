//! Browser worker/main-thread protocol metadata.
//!
//! The Rust crate owns the browser worker/main-thread command names and transfer
//! metadata used by the Wasm bindings.

mod names;
#[cfg(all(
    feature = "browser-worker",
    target_family = "wasm",
    target_os = "unknown"
))]
mod transfers;

#[cfg(any(
    test,
    all(
        feature = "browser-main-thread",
        target_family = "wasm",
        target_os = "unknown"
    ),
    all(
        feature = "browser-worker",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
pub(crate) use names::{
    BENCHMARK_ECHO_OPEN_COMMAND, BENCHMARK_LATENCY_COMMAND, BENCHMARK_THROUGHPUT_COMMAND,
    MAIN_RTC_ACCEPT_ANSWER_COMMAND, MAIN_RTC_ACCEPT_OFFER_COMMAND, MAIN_RTC_ADD_REMOTE_ICE_COMMAND,
    MAIN_RTC_CLOSE_PEER_CONNECTION_COMMAND, MAIN_RTC_CREATE_ANSWER_COMMAND,
    MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND, MAIN_RTC_CREATE_OFFER_COMMAND,
    MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND, MAIN_RTC_NEXT_LOCAL_ICE_COMMAND,
    MAIN_RTC_TAKE_DATA_CHANNEL_COMMAND, STREAM_ACCEPT_BI_COMMAND, STREAM_CANCEL_PENDING_COMMAND,
    STREAM_CLOSE_SEND_COMMAND, STREAM_OPEN_BI_COMMAND, STREAM_RECEIVE_CHUNK_COMMAND,
    STREAM_RESET_COMMAND, STREAM_SEND_CHUNK_COMMAND, WORKER_ACCEPT_CLOSE_COMMAND,
    WORKER_ACCEPT_NEXT_COMMAND, WORKER_ACCEPT_OPEN_COMMAND, WORKER_ATTACH_DATA_CHANNEL_COMMAND,
    WORKER_CONNECTION_CLOSE_COMMAND, WORKER_DIAL_COMMAND, WORKER_NODE_CLOSE_COMMAND,
    WORKER_SPAWN_COMMAND,
};
#[cfg(any(
    test,
    all(
        feature = "browser-worker",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
pub(crate) use names::{WORKER_BOOTSTRAP_SIGNAL_COMMAND, WORKER_MAIN_RTC_RESULT_COMMAND};
#[cfg(any(
    test,
    all(
        feature = "browser-main-thread",
        target_family = "wasm",
        target_os = "unknown"
    ),
    all(
        feature = "browser-worker",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
pub(crate) use names::{WORKER_PROTOCOL_COMMAND_COMMAND, WORKER_PROTOCOL_NEXT_EVENT_COMMAND};
#[cfg(all(
    feature = "browser-worker",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(crate) use transfers::response_transfer_requirements_for_command;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(crate) fn encode_js_value<T: serde::Serialize>(
    value: &T,
) -> std::result::Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> {
    serde_wasm_bindgen::to_value(value).map_err(|err| {
        wasm_bindgen::JsValue::from_str(&format!("failed to encode browser protocol value: {err}"))
    })
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(crate) fn decode_js_value<T: serde::de::DeserializeOwned>(
    value: wasm_bindgen::JsValue,
) -> std::result::Result<T, wasm_bindgen::JsValue> {
    serde_wasm_bindgen::from_value(value).map_err(|err| {
        wasm_bindgen::JsValue::from_str(&format!("failed to decode browser protocol value: {err}"))
    })
}
