use crate::error::{Error, Result};

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
use {
    wasm_bindgen::JsValue,
    web_sys::{RtcDataChannel, RtcDataChannelState},
};

pub(super) const MAX_SESSION_KEY_LEN: usize = 1024;

pub(super) fn validate_session_key(session_key: &str) -> Result<()> {
    if session_key.is_empty() {
        return Err(Error::InvalidConfig("session key must not be empty"));
    }
    if session_key.len() > MAX_SESSION_KEY_LEN {
        return Err(Error::InvalidConfig("session key is too long"));
    }
    if session_key.chars().any(char::is_control) {
        return Err(Error::InvalidConfig(
            "session key must not contain control characters",
        ));
    }
    Ok(())
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn rtc_peer_connection_available() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("RTCPeerConnection"))
        .map(|value| value.is_function())
        .unwrap_or(false)
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn rtc_data_channel_constructor_available() -> bool {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("RTCDataChannel"))
        .map(|value| value.is_function())
        .unwrap_or(false)
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn validate_rtc_peer_connection_available() -> Result<()> {
    if rtc_peer_connection_available() {
        Ok(())
    } else {
        Err(Error::BrowserWebRtcUnavailable)
    }
}

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(super) fn validate_data_channel_transferable(channel: &RtcDataChannel) -> Result<()> {
    if !rtc_data_channel_constructor_available() {
        return Err(Error::BrowserWebRtcUnavailable);
    }
    match channel.ready_state() {
        RtcDataChannelState::Connecting | RtcDataChannelState::Open => Ok(()),
        state => Err(Error::WebRtc(format!(
            "RTCDataChannel cannot be transferred after it reaches {state:?}"
        ))),
    }
}
