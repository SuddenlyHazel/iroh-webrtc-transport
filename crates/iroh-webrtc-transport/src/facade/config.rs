//! Shared facade configuration for browser and native node helpers.

use crate::{
    config::WebRtcSessionConfig, error::Result, facade::WebRtcDialOptions,
    transport::WebRtcTransportConfig,
};

/// Target-neutral configuration shell for future app facade node builders.
#[derive(Debug, Clone)]
pub struct WebRtcNodeConfig {
    /// Per-peer WebRTC session tuning.
    pub session: WebRtcSessionConfig,
    /// Packet transport bridge configuration.
    pub transport: WebRtcTransportConfig,
    /// Default dial preference used when a call does not override it.
    pub default_dial_options: WebRtcDialOptions,
}

impl WebRtcNodeConfig {
    /// Validate the node, session, and transport configuration.
    pub fn validate(&self) -> Result<()> {
        self.session.validate()?;
        self.transport.queues.validate()?;
        self.transport.frame.validate()
    }
}

impl Default for WebRtcNodeConfig {
    fn default() -> Self {
        Self {
            session: WebRtcSessionConfig::default(),
            transport: WebRtcTransportConfig::default(),
            default_dial_options: WebRtcDialOptions::default(),
        }
    }
}
