//! Target-neutral application facade contracts.
//!
//! These types describe the Rust-owned app facade shape without tying callers
//! to browser worker internals, Wasm bindings, or the native WebRTC backend.

mod config;
mod dial;
#[cfg(all(
    feature = "native",
    not(all(target_family = "wasm", target_os = "unknown"))
))]
mod native;

pub use config::WebRtcNodeConfig;
pub use dial::WebRtcDialOptions;
#[cfg(all(
    feature = "native",
    not(all(target_family = "wasm", target_os = "unknown"))
))]
pub use native::NativeWebRtcIrohNode;
