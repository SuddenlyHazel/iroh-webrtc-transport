use crate::core::signaling::BootstrapTransportIntent;

/// Dial-time transport preference for the application facade.
///
/// This wraps the existing bootstrap transport intent so app-level code can
/// pass one facade options value before the native/browser orchestration exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WebRtcDialOptions {
    /// Desired transport behavior for the WebRTC bootstrap attempt.
    pub transport_intent: BootstrapTransportIntent,
}

impl WebRtcDialOptions {
    /// Creates dial options from an existing bootstrap transport intent.
    pub const fn new(transport_intent: BootstrapTransportIntent) -> Self {
        Self { transport_intent }
    }

    /// Prefer WebRTC and allow Iroh relay fallback if WebRTC cannot connect.
    pub const fn webrtc_preferred() -> Self {
        Self::new(BootstrapTransportIntent::WebRtcPreferred)
    }

    /// Require the WebRTC path.
    pub const fn webrtc_only() -> Self {
        Self::new(BootstrapTransportIntent::WebRtcOnly)
    }

    /// Use the normal Iroh relay path without attempting WebRTC.
    pub const fn iroh_relay() -> Self {
        Self::new(BootstrapTransportIntent::IrohRelay)
    }
}

impl Default for WebRtcDialOptions {
    fn default() -> Self {
        Self::webrtc_preferred()
    }
}

impl From<BootstrapTransportIntent> for WebRtcDialOptions {
    fn from(transport_intent: BootstrapTransportIntent) -> Self {
        Self::new(transport_intent)
    }
}

impl From<WebRtcDialOptions> for BootstrapTransportIntent {
    fn from(options: WebRtcDialOptions) -> Self {
        options.transport_intent
    }
}
