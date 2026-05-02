//! Browser/wasm entry points.
//!
//! The browser architecture is currently single-runtime: the main-thread Wasm
//! instance owns Iroh endpoint/session state, WebRTC negotiation, and
//! `RTCDataChannel` packet I/O directly.

pub use crate::{
    config::{WebRtcIceConfig, WebRtcSessionConfig},
    core::{
        hub::{LocalIceEvent, LocalIceQueue, WebRtcIceCandidate},
        signaling::WEBRTC_DATA_CHANNEL_LABEL,
    },
};

/// Browser-facing alias for the shared per-session WebRTC configuration.
pub type BrowserWebRtcSessionConfig = WebRtcSessionConfig;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
use {
    std::{
        cell::RefCell,
        collections::{HashMap, VecDeque},
        rc::Rc,
    },
    tokio::sync::oneshot,
    wasm_bindgen::{JsCast, JsValue, closure::Closure},
    wasm_bindgen_futures::JsFuture,
    web_sys::{
        RtcConfiguration, RtcDataChannel, RtcDataChannelEvent, RtcDataChannelInit,
        RtcDataChannelType, RtcIceCandidateInit, RtcPeerConnection, RtcPeerConnectionIceEvent,
        RtcSdpType, RtcSessionDescriptionInit,
    },
};

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod registry;
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(crate) use registry::{BrowserRtcRegistry, BrowserRtcSessionRole};

#[cfg(any(
    test,
    all(
        feature = "browser-main-thread",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
mod capabilities;

#[cfg(any(
    test,
    all(
        feature = "browser-main-thread",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
mod js_boundary;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod facade;

#[cfg(any(
    test,
    all(
        feature = "browser-main-thread",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
mod protocol;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod rtc;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub use crate::browser_console_tracing::install_browser_console_tracing;
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub use crate::browser_runtime::BrowserProtocolRegistry;
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(crate) use crate::browser_runtime::BrowserRuntime;
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub use facade::{
    BrowserDialOptions, BrowserDialTransportPreference, BrowserProtocolHandle,
    BrowserWebRtcAcceptor, BrowserWebRtcConnection, BrowserWebRtcNode, BrowserWebRtcNodeBuilder,
    BrowserWebRtcNodeConfig, BrowserWebRtcStream,
};
#[cfg(any(
    test,
    all(
        feature = "browser-main-thread",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
pub use protocol::BrowserProtocol;
#[cfg(all(
    test,
    not(all(
        feature = "browser-main-thread",
        target_family = "wasm",
        target_os = "unknown"
    ))
))]
mod tests {
    use super::capabilities::{MAX_SESSION_KEY_LEN, validate_session_key};
    use crate::error::Error;

    #[test]
    fn session_key_validation_rejects_empty_long_and_control_keys() {
        assert!(validate_session_key("session-1").is_ok());
        assert!(matches!(
            validate_session_key(""),
            Err(Error::InvalidConfig(_))
        ));
        assert!(matches!(
            validate_session_key("session\n1"),
            Err(Error::InvalidConfig(_))
        ));
        assert!(matches!(
            validate_session_key(&"x".repeat(MAX_SESSION_KEY_LEN + 1)),
            Err(Error::InvalidConfig(_))
        ));
    }
}
