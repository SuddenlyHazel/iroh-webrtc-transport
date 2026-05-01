use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid WebRTC custom address: {0}")]
    InvalidAddr(&'static str),

    #[error("invalid WebRTC frame: {0}")]
    InvalidFrame(&'static str),

    #[error("payload length {actual} exceeds maximum {max}")]
    PayloadTooLarge { actual: usize, max: usize },

    #[error("unknown WebRTC session")]
    UnknownSession,

    #[error("WebRTC session is closed")]
    SessionClosed,

    #[error("WebRTC send queue is full")]
    SendQueueFull,

    #[error("browser WebRTC APIs are only available on wasm32-unknown-unknown")]
    BrowserWebRtcUnavailable,

    #[error("browser WebRTC operation failed: {0}")]
    WebRtc(String),

    #[error("invalid WebRTC ICE configuration: {0}")]
    InvalidIceConfig(&'static str),

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

pub type Result<T, E = Error> = std::result::Result<T, E>;
