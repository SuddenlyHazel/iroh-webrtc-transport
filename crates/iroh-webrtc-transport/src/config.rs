//! Configuration types for the WebRTC custom transport.
//!
//! Start here when tuning how the Rust transport behaves. These types are
//! shared by the browser runtime and native/server entry points.

use crate::error::{Error, Result};
pub use crate::transport::{WebRtcQueueConfig, WebRtcTransportConfig};

/// Default number of local ICE events buffered per native or browser WebRTC session.
pub const DEFAULT_LOCAL_ICE_QUEUE_CAPACITY: usize = 128;
/// Default per-session packet queue capacity for inbound and outbound custom transport packets.
pub const DEFAULT_SESSION_QUEUE_CAPACITY: usize = 1024;
/// Default DataChannel buffered amount that resumes a paused outbound pump.
pub const DEFAULT_DATA_CHANNEL_BUFFERED_AMOUNT_LOW_THRESHOLD: usize = 4 * 1024 * 1024;
/// Default DataChannel buffered amount that pauses a per-session outbound pump.
pub const DEFAULT_DATA_CHANNEL_BUFFERED_AMOUNT_HIGH_THRESHOLD: usize = 16 * 1024 * 1024;

/// Direct-only ICE server configuration.
///
/// This intentionally models STUN only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcIceConfig {
    /// STUN or STUNS URLs used by WebRTC ICE candidate gathering.
    pub stun_urls: Vec<String>,
}

impl WebRtcIceConfig {
    /// Build a direct-only ICE config from STUN URLs.
    ///
    /// TURN is intentionally rejected. This crate's relay fallback is Iroh's
    /// normal dial path, so WebRTC ICE configuration is kept to candidate
    /// discovery for true peer-to-peer attempts.
    pub fn direct_only(stun_urls: impl IntoIterator<Item = impl Into<String>>) -> Result<Self> {
        let stun_urls = stun_urls
            .into_iter()
            .map(Into::into)
            .map(validate_stun_url)
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { stun_urls })
    }

    /// Disable configured ICE servers.
    ///
    /// This leaves WebRTC with host/local candidates only. It is useful for
    /// localhost tests, same-LAN peers, offline environments, or controlled
    /// deployments where external STUN traffic is undesirable. It is not the
    /// normal best-effort NAT traversal path; use the default direct-STUN config
    /// when peers may be behind NAT.
    pub fn no_ice_servers() -> Self {
        Self {
            stun_urls: Vec::new(),
        }
    }

    /// Build the default direct-STUN configuration.
    pub fn default_direct_stun() -> Self {
        Self {
            stun_urls: vec!["stun:stun.l.google.com:19302".into()],
        }
    }

    /// Validate that all configured ICE server URLs are STUN or STUNS URLs.
    pub fn validate_direct_only(&self) -> Result<()> {
        for url in &self.stun_urls {
            validate_stun_url(url.clone())?;
        }
        Ok(())
    }
}

impl Default for WebRtcIceConfig {
    fn default() -> Self {
        Self::default_direct_stun()
    }
}

/// Packet frame sizing for the RTCDataChannel carrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebRtcFrameConfig {
    /// Maximum payload bytes accepted inside one `WebRtcPacketFrame`.
    pub max_payload_len: usize,
}

impl WebRtcFrameConfig {
    /// Validate the packet frame size limits.
    pub fn validate(&self) -> Result<()> {
        if self.max_payload_len == 0 || self.max_payload_len > u16::MAX as usize {
            return Err(Error::InvalidConfig(
                "max payload length must be between 1 and u16::MAX",
            ));
        }
        Ok(())
    }
}

impl Default for WebRtcFrameConfig {
    fn default() -> Self {
        Self {
            max_payload_len: crate::core::frame::DEFAULT_MAX_PAYLOAD_LEN,
        }
    }
}

/// Outbound RTCDataChannel backpressure tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebRtcDataChannelConfig {
    /// Resume a paused per-session outbound pump when the DataChannel drains
    /// to this threshold.
    pub buffered_amount_low_threshold: usize,
    /// Pause a per-session outbound pump when sending the next frame would
    /// put the DataChannel above this threshold.
    pub buffered_amount_high_threshold: usize,
}

impl WebRtcDataChannelConfig {
    /// Validate the DataChannel buffered amount thresholds.
    pub fn validate(&self) -> Result<()> {
        if self.buffered_amount_low_threshold > u32::MAX as usize
            || self.buffered_amount_high_threshold > u32::MAX as usize
        {
            return Err(Error::InvalidConfig(
                "buffered amount thresholds must fit in u32",
            ));
        }
        if self.buffered_amount_low_threshold >= self.buffered_amount_high_threshold {
            return Err(Error::InvalidConfig(
                "buffered amount low threshold must be lower than high threshold",
            ));
        }
        Ok(())
    }
}

impl Default for WebRtcDataChannelConfig {
    fn default() -> Self {
        Self {
            buffered_amount_low_threshold: DEFAULT_DATA_CHANNEL_BUFFERED_AMOUNT_LOW_THRESHOLD,
            buffered_amount_high_threshold: DEFAULT_DATA_CHANNEL_BUFFERED_AMOUNT_HIGH_THRESHOLD,
        }
    }
}

/// Full per-peer WebRTC session tuning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcSessionConfig {
    /// STUN-only ICE candidate gathering configuration.
    pub ice: WebRtcIceConfig,
    /// DataChannel packet frame sizing.
    pub frame: WebRtcFrameConfig,
    /// DataChannel send-side backpressure thresholds.
    pub data_channel: WebRtcDataChannelConfig,
    /// Number of local ICE candidate events buffered before the signaling side
    /// drains them.
    pub local_ice_queue_capacity: usize,
}

impl WebRtcSessionConfig {
    /// Build a session config with explicit direct-STUN URLs.
    pub fn direct_stun(stun_urls: impl IntoIterator<Item = impl Into<String>>) -> Result<Self> {
        Ok(Self {
            ice: WebRtcIceConfig::direct_only(stun_urls)?,
            ..Self::default()
        })
    }

    /// Build a session config using the crate's default direct-STUN server.
    pub fn default_direct_stun() -> Self {
        Self {
            ice: WebRtcIceConfig::default_direct_stun(),
            ..Self::default()
        }
    }

    /// Build a session config with no configured ICE servers.
    ///
    /// This opts out of the crate's default STUN-assisted ICE gathering. WebRTC
    /// will only gather host/local candidates, so connectivity is generally
    /// limited to localhost, same-LAN, or otherwise directly reachable peers.
    /// Use this only when that limitation is intentional; the default config is
    /// the working best-effort P2P path across NAT.
    pub fn no_ice_servers() -> Self {
        Self {
            ice: WebRtcIceConfig::no_ice_servers(),
            ..Self::default()
        }
    }

    /// Validate all per-session tuning knobs.
    pub fn validate(&self) -> Result<()> {
        self.ice.validate_direct_only()?;
        self.frame.validate()?;
        self.data_channel.validate()?;
        if self.local_ice_queue_capacity == 0 {
            return Err(Error::InvalidConfig(
                "local ICE queue capacity must be greater than zero",
            ));
        }
        Ok(())
    }
}

impl Default for WebRtcSessionConfig {
    fn default() -> Self {
        Self {
            ice: WebRtcIceConfig::default(),
            frame: WebRtcFrameConfig::default(),
            data_channel: WebRtcDataChannelConfig::default(),
            local_ice_queue_capacity: DEFAULT_LOCAL_ICE_QUEUE_CAPACITY,
        }
    }
}

fn validate_stun_url(url: String) -> Result<String> {
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("stun:") && !lower.starts_with("stuns:") {
        return Err(Error::InvalidIceConfig(
            "direct-only WebRTC sessions only accept stun: or stuns: ICE server URLs",
        ));
    }
    if lower.starts_with("turn:") || lower.starts_with("turns:") {
        return Err(Error::InvalidIceConfig(
            "TURN relays are disabled for direct-only WebRTC sessions",
        ));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_ice_config_accepts_stun_and_rejects_turn() {
        let config = WebRtcIceConfig::direct_only([
            "stun:stun.example.test:3478",
            "stuns:stun.example.test",
        ])
        .unwrap();
        assert_eq!(config.stun_urls.len(), 2);

        assert!(matches!(
            WebRtcIceConfig::direct_only(["turn:turn.example.test:3478"]),
            Err(Error::InvalidIceConfig(_))
        ));
        assert!(matches!(
            WebRtcIceConfig::direct_only(["https://example.test"]),
            Err(Error::InvalidIceConfig(_))
        ));
    }

    #[test]
    fn session_config_default_uses_direct_stun_and_no_ice_is_explicit() {
        let default = WebRtcSessionConfig::default();
        assert_eq!(default.ice.stun_urls, ["stun:stun.l.google.com:19302"]);
        assert!(default.validate().is_ok());

        let direct_stun = WebRtcSessionConfig::default_direct_stun();
        assert_eq!(direct_stun.ice.stun_urls, ["stun:stun.l.google.com:19302"]);
        assert!(direct_stun.validate().is_ok());

        let no_ice = WebRtcSessionConfig::no_ice_servers();
        assert!(no_ice.ice.stun_urls.is_empty());
        assert!(no_ice.validate().is_ok());
    }

    #[test]
    fn session_config_validates_tuning_knobs() {
        let valid = WebRtcSessionConfig::default();
        assert!(valid.validate().is_ok());

        assert!(matches!(
            WebRtcSessionConfig {
                frame: WebRtcFrameConfig { max_payload_len: 0 },
                ..WebRtcSessionConfig::default()
            }
            .validate(),
            Err(Error::InvalidConfig(_))
        ));
        assert!(matches!(
            WebRtcSessionConfig {
                data_channel: WebRtcDataChannelConfig {
                    buffered_amount_low_threshold: u32::MAX as usize + 1,
                    ..WebRtcDataChannelConfig::default()
                },
                ..WebRtcSessionConfig::default()
            }
            .validate(),
            Err(Error::InvalidConfig(_))
        ));
        assert!(matches!(
            WebRtcSessionConfig {
                data_channel: WebRtcDataChannelConfig {
                    buffered_amount_low_threshold: 4096,
                    buffered_amount_high_threshold: 4096,
                },
                ..WebRtcSessionConfig::default()
            }
            .validate(),
            Err(Error::InvalidConfig(_))
        ));
    }
}
