//! Error types for WebRTC transport setup, framing, and packet routing.

use std::io;

/// Error returned by transport, configuration, browser, and native helpers.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A custom transport address did not match the WebRTC address format.
    #[error("invalid WebRTC custom address: {0}")]
    InvalidAddr(&'static str),

    /// A DataChannel packet frame was malformed or unsupported.
    #[error("invalid WebRTC frame: {0}")]
    InvalidFrame(&'static str),

    /// A packet payload exceeded the configured frame limit.
    #[error("payload length {actual} exceeds maximum {max}")]
    PayloadTooLarge {
        /// Actual payload length in bytes.
        actual: usize,
        /// Maximum accepted payload length in bytes.
        max: usize,
    },

    /// The requested WebRTC session is not known to the session hub.
    #[error("unknown WebRTC session")]
    UnknownSession,

    /// The requested WebRTC session or hub has closed.
    #[error("WebRTC session is closed")]
    SessionClosed,

    /// The outbound packet queue cannot accept more packets.
    #[error("WebRTC send queue is full")]
    SendQueueFull,

    /// Browser WebRTC APIs are unavailable for the active target.
    #[error("browser WebRTC APIs are only available on wasm32-unknown-unknown")]
    BrowserWebRtcUnavailable,

    /// A browser or native WebRTC operation failed.
    #[error("browser WebRTC operation failed: {0}")]
    WebRtc(String),

    /// The ICE configuration is not valid for this crate's STUN-only model.
    #[error("invalid WebRTC ICE configuration: {0}")]
    InvalidIceConfig(&'static str),

    /// General transport or facade configuration is invalid.
    #[error("invalid WebRTC configuration: {0}")]
    InvalidConfig(&'static str),
}

impl From<Error> for io::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::InvalidAddr(_) | Error::InvalidFrame(_) | Error::PayloadTooLarge { .. } => {
                io::Error::new(io::ErrorKind::InvalidInput, value)
            }
            Error::UnknownSession | Error::SessionClosed => {
                io::Error::new(io::ErrorKind::NotConnected, value)
            }
            Error::SendQueueFull => io::Error::new(io::ErrorKind::WouldBlock, value),
            Error::BrowserWebRtcUnavailable => io::Error::new(io::ErrorKind::Unsupported, value),
            Error::WebRtc(_) | Error::InvalidIceConfig(_) | Error::InvalidConfig(_) => {
                io::Error::other(value)
            }
        }
    }
}

/// Result alias using [`Error`] by default.
pub type Result<T, E = Error> = std::result::Result<T, E>;
