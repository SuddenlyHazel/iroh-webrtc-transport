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
//! - [`browser`]: browser Wasm node facade.
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
//! Browser applications should use the browser `BrowserWebRtcNode` facade. The
//! facade owns Iroh/Wasm endpoint state and browser WebRTC objects in the same
//! main-thread runtime.

pub mod browser;
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod browser_console_tracing;
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod browser_runtime;
pub mod config;
mod core;
/// Error and result types used by the transport, facades, and session helpers.
pub mod error;
pub mod facade;
#[cfg(all(
    feature = "native",
    not(all(target_family = "wasm", target_os = "unknown"))
))]
pub mod native;
/// Iroh custom transport integration for WebRTC-backed packet I/O.
pub mod transport;
pub use error::{Error, Result};
#[cfg(all(
    feature = "native",
    not(all(target_family = "wasm", target_os = "unknown"))
))]
pub use facade::NativeWebRtcIrohNode;
pub use facade::{WebRtcDialOptions, WebRtcNodeConfig};

/// Export a browser application entry point from an application Wasm module.
#[macro_export]
macro_rules! browser_app {
    (
        app = $app_entry:ident => $app_impl:path $(;)?
    ) => {
        #[::wasm_bindgen::prelude::wasm_bindgen]
        pub async fn $app_entry() -> ::std::result::Result<(), ::wasm_bindgen::JsValue> {
            $app_impl().await
        }
    };
}
