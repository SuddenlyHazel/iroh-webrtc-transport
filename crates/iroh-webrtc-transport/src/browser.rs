//! Browser/wasm entry points.
//!
//! The canonical browser architecture keeps `RTCPeerConnection`, SDP/ICE, and
//! `RTCDataChannel` creation on the main thread for browser compatibility. A worker Wasm runtime owns Iroh endpoint/session state. A
//! dedicated `MessagePort` carries RTC control messages between them, and the
//! product path requires the opened `RTCDataChannel` to be transferred to the
//! worker before packet I/O starts.

#[cfg(all(test, not(all(target_family = "wasm", target_os = "unknown"))))]
use crate::error::Error;
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
use crate::browser_protocol::{
    BENCHMARK_ECHO_OPEN_COMMAND, BENCHMARK_LATENCY_COMMAND, BENCHMARK_THROUGHPUT_COMMAND,
    MAIN_RTC_ACCEPT_ANSWER_COMMAND, MAIN_RTC_ACCEPT_OFFER_COMMAND, MAIN_RTC_ADD_REMOTE_ICE_COMMAND,
    MAIN_RTC_CLOSE_PEER_CONNECTION_COMMAND, MAIN_RTC_CREATE_ANSWER_COMMAND,
    MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND, MAIN_RTC_CREATE_OFFER_COMMAND,
    MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND, MAIN_RTC_NEXT_LOCAL_ICE_COMMAND,
    MAIN_RTC_TAKE_DATA_CHANNEL_COMMAND, STREAM_ACCEPT_BI_COMMAND, STREAM_CLOSE_SEND_COMMAND,
    STREAM_OPEN_BI_COMMAND, STREAM_RECEIVE_CHUNK_COMMAND, STREAM_SEND_CHUNK_COMMAND,
    WORKER_ACCEPT_CLOSE_COMMAND, WORKER_ACCEPT_NEXT_COMMAND, WORKER_ACCEPT_OPEN_COMMAND,
    WORKER_ATTACH_DATA_CHANNEL_COMMAND, WORKER_CONNECTION_CLOSE_COMMAND, WORKER_DIAL_COMMAND,
    WORKER_NODE_CLOSE_COMMAND, WORKER_SPAWN_COMMAND,
};
#[cfg(all(test, not(all(target_family = "wasm", target_os = "unknown"))))]
use crate::browser_protocol::{
    MAIN_RTC_ACCEPT_ANSWER_COMMAND, MAIN_RTC_ACCEPT_OFFER_COMMAND, MAIN_RTC_ADD_REMOTE_ICE_COMMAND,
    MAIN_RTC_CLOSE_PEER_CONNECTION_COMMAND, MAIN_RTC_CREATE_ANSWER_COMMAND,
    MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND, MAIN_RTC_CREATE_OFFER_COMMAND,
    MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND, MAIN_RTC_NEXT_LOCAL_ICE_COMMAND,
};

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
    wasm_bindgen_futures::{JsFuture, spawn_local},
    web_sys::{
        MessageEvent, MessagePort, RtcConfiguration, RtcDataChannel, RtcDataChannelEvent,
        RtcDataChannelInit, RtcDataChannelType, RtcIceCandidateInit, RtcPeerConnection,
        RtcPeerConnectionIceEvent, RtcSdpType, RtcSessionDescriptionInit, Worker,
    },
};

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod registry;

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
mod js_wire;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod facade;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod single_wasm_worker;

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
mod rtc_control_wire;

#[cfg(any(
    test,
    all(
        feature = "browser-main-thread",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
mod worker_bridge_wire;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod worker_bridge;

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub use crate::browser_console_tracing::install_browser_console_tracing;
#[cfg(all(
    feature = "browser-worker",
    target_family = "wasm",
    target_os = "unknown"
))]
pub use crate::browser_worker::start_browser_worker;
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub use facade::{
    BrowserDialOptions, BrowserDialTransportPreference, BrowserWebRtcAcceptor,
    BrowserWebRtcConnection, BrowserWebRtcNode, BrowserWebRtcNodeConfig, BrowserWebRtcStream,
};
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub use single_wasm_worker::{
    create_current_wasm_worker_factory, create_single_wasm_worker_factory,
};

#[cfg(all(
    test,
    not(all(
        feature = "browser-main-thread",
        target_family = "wasm",
        target_os = "unknown"
    ))
))]
mod tests {
    use super::*;
    use super::{
        capabilities::{MAX_SESSION_KEY_LEN, validate_session_key},
        js_wire::wire_error_code_for_command,
        worker_bridge_wire::BrowserWorkerRequestTracker,
    };

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

    #[test]
    fn main_rtc_command_names_match_canonical_protocol() {
        assert_eq!(
            MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND,
            "main.create-peer-connection"
        );
        assert_eq!(
            MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND,
            "main.create-data-channel"
        );
        assert_eq!(MAIN_RTC_CREATE_OFFER_COMMAND, "main.create-offer");
        assert_eq!(MAIN_RTC_ACCEPT_OFFER_COMMAND, "main.accept-offer");
        assert_eq!(MAIN_RTC_CREATE_ANSWER_COMMAND, "main.create-answer");
        assert_eq!(MAIN_RTC_ACCEPT_ANSWER_COMMAND, "main.accept-answer");
        assert_eq!(MAIN_RTC_NEXT_LOCAL_ICE_COMMAND, "main.next-local-ice");
        assert_eq!(MAIN_RTC_ADD_REMOTE_ICE_COMMAND, "main.add-remote-ice");
        assert_eq!(
            MAIN_RTC_CLOSE_PEER_CONNECTION_COMMAND,
            "main.close-peer-connection"
        );
    }

    #[test]
    fn wire_error_codes_are_canonical_protocol_codes() {
        assert_eq!(
            wire_error_code_for_command(
                Some(MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND),
                &Error::BrowserWebRtcUnavailable,
            ),
            "unsupportedBrowser"
        );
        assert_eq!(
            wire_error_code_for_command(
                Some(MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND),
                &Error::InvalidIceConfig("bad stun"),
            ),
            "invalidStunUrl"
        );
        assert_eq!(
            wire_error_code_for_command(
                Some(MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND),
                &Error::WebRtc("failed".into()),
            ),
            "dataChannelFailed"
        );
        assert_eq!(
            wire_error_code_for_command(
                Some(MAIN_RTC_CREATE_OFFER_COMMAND),
                &Error::UnknownSession,
            ),
            "webrtcFailed"
        );
        assert_eq!(
            wire_error_code_for_command(
                Some(MAIN_RTC_NEXT_LOCAL_ICE_COMMAND),
                &Error::SessionClosed,
            ),
            "closed"
        );
    }

    #[test]
    fn worker_request_tracker_allocates_monotonic_ids_and_tracks_pending() {
        let mut tracker = BrowserWorkerRequestTracker::default();

        assert_eq!(tracker.allocate().unwrap(), 1);
        assert_eq!(tracker.allocate().unwrap(), 2);
        assert_eq!(tracker.pending_len(), 2);
        assert!(tracker.complete(1));
        assert!(!tracker.complete(1));
        assert_eq!(tracker.pending_len(), 1);

        assert_eq!(tracker.allocate().unwrap(), 3);
        let failed = tracker.fail_all();
        assert_eq!(failed, vec![2, 3]);
        assert_eq!(tracker.pending_len(), 0);
    }
}
