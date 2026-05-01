//! WebRTC-backed custom transport primitives for Iroh.
//!
//! This crate provides the reusable Rust pieces for establishing a direct
//! WebRTC `RTCDataChannel` path and promoting that path into an Iroh custom
//! transport session. Application protocols, demos, and UI state live outside
//! the crate.
//!
//! # Start Here
//!
//! Most code should import one of the grouped public modules:
//!
//! - [`config`]: STUN, frame, and queue tuning.
//! - [`browser`]: browser Wasm node facade and worker helpers.
//! - [`native`]: native/server WebRTC session backend.
//! - [`transport`]: Iroh custom transport integration.
//! - [`facade`]: target-neutral facade configuration and dial options.
//!
//! # Native/server shape
//!
//! ```no_run
//! use iroh_webrtc_transport::{
//!     config::WebRtcTransportConfig,
//!     transport::{WebRtcTransport, configure_endpoint},
//! };
//!
//! # fn configure(builder: iroh::endpoint::Builder) -> iroh::endpoint::Builder {
//! let transport = WebRtcTransport::new(WebRtcTransportConfig::default());
//! let hub = transport.session_hub();
//! let builder = configure_endpoint(builder, transport);
//! # let _ = hub;
//! builder
//! # }
//! ```
//!
//! With the `native` feature enabled, use [`native::NativeWebRtcSession`] to
//! answer or initiate the WebRTC PeerConnection and attach the resulting
//! DataChannel to the `SessionHub`.
//!
//! # Browser shape
//!
//! Browser applications should use [`browser::BrowserWebRtcNode`]. The facade
//! keeps Iroh/Wasm access serialized in a worker while the crate's internal
//! main-thread bridge owns `RTCPeerConnection`, SDP, ICE, and
//! `RTCDataChannel` creation. A dedicated `MessagePort` carries RTC control
//! messages between the main thread and the worker, and the resulting
//! `RTCDataChannel` is transferred to the worker before packet I/O starts.

pub mod browser;
#[cfg(all(
    any(feature = "browser-main-thread", feature = "browser-worker"),
    target_family = "wasm",
    target_os = "unknown"
))]
mod browser_console_tracing;
#[doc(hidden)]
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
pub mod browser_protocol;
#[cfg(any(
    test,
    all(
        feature = "browser-worker",
        target_family = "wasm",
        target_os = "unknown"
    )
))]
mod browser_worker;
pub mod config;
mod core;
pub mod error;
pub mod facade;
#[cfg(all(
    feature = "native",
    not(all(target_family = "wasm", target_os = "unknown"))
))]
pub mod native;
pub mod transport;
pub use error::{Error, Result};
#[cfg(all(
    feature = "native",
    not(all(target_family = "wasm", target_os = "unknown"))
))]
pub use facade::NativeWebRtcIrohNode;
pub use facade::{WebRtcDialOptions, WebRtcNodeConfig};

/// Export a browser worker entry point from an application Wasm module.
///
/// Browser Rust applications can use this macro to ship one Wasm module that
/// contains both their main-thread app entry point and the Iroh WebRTC worker
/// entry point.
#[macro_export]
macro_rules! browser_app {
    (
        app = $app_entry:ident => $app_impl:path;
        worker = $worker_entry:ident $(;)?
    ) => {
        #[::wasm_bindgen::prelude::wasm_bindgen]
        pub async fn $app_entry() -> ::std::result::Result<(), ::wasm_bindgen::JsValue> {
            $app_impl().await
        }

        $crate::browser_app! {
            worker = $worker_entry;
        }
    };
    (worker = $worker_entry:ident $(;)?) => {
        #[::wasm_bindgen::prelude::wasm_bindgen]
        pub fn $worker_entry() -> ::std::result::Result<(), ::wasm_bindgen::JsValue> {
            $crate::browser::start_browser_worker()
        }

        #[::wasm_bindgen::prelude::wasm_bindgen(js_name = __iroh_webrtc_wasm_module)]
        pub fn __iroh_webrtc_wasm_module() -> ::wasm_bindgen::JsValue {
            ::wasm_bindgen::module()
        }
    };
}
