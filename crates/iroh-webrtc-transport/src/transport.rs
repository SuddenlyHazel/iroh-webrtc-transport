//! Iroh custom transport integration for WebRTC-backed packet delivery.

use std::{io, num::NonZeroUsize, sync::Arc};

use iroh::endpoint::{
    Builder,
    transports::{CustomEndpoint, CustomTransport},
};

use crate::{
    config::{DEFAULT_SESSION_QUEUE_CAPACITY, WebRtcFrameConfig},
    core::{endpoint::WebRtcEndpoint, hub::SessionHub},
    error::{Error, Result},
};

/// Queue capacities for the packet-oriented custom transport bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebRtcQueueConfig {
    /// Number of decoded DataChannel packet frames buffered for Iroh/noq reads.
    pub recv_capacity: usize,
    /// Number of Iroh/noq outbound packets buffered before the DataChannel pump
    /// drains them.
    pub send_capacity: usize,
}

impl WebRtcQueueConfig {
    /// Validate that both packet queues can hold at least one packet.
    pub fn validate(&self) -> Result<()> {
        if self.recv_capacity == 0 || self.send_capacity == 0 {
            return Err(Error::InvalidConfig(
                "WebRTC transport queue capacities must be greater than zero",
            ));
        }
        Ok(())
    }
}

impl Default for WebRtcQueueConfig {
    fn default() -> Self {
        Self {
            recv_capacity: DEFAULT_SESSION_QUEUE_CAPACITY,
            send_capacity: DEFAULT_SESSION_QUEUE_CAPACITY,
        }
    }
}

/// Configuration for the WebRTC custom transport registered on an Iroh endpoint.
#[derive(Debug, Clone)]
pub struct WebRtcTransportConfig {
    /// Optional local virtual WebRTC custom address advertised to Iroh/noq.
    pub local_addr: Option<iroh_base::CustomAddr>,
    /// Packet queue capacities for the custom transport endpoint.
    pub queues: WebRtcQueueConfig,
    /// Packet frame sizing enforced by custom senders.
    pub frame: WebRtcFrameConfig,
    /// Maximum number of NOQ datagrams to batch into one custom transport
    /// transmit.
    pub max_transmit_segments: NonZeroUsize,
}

/// Iroh custom transport that routes WebRTC packets through a shared session hub.
#[derive(Debug, Clone)]
pub struct WebRtcTransport {
    config: WebRtcTransportConfig,
    hub: SessionHub,
    local_addrs: n0_watcher::Watchable<Vec<iroh_base::CustomAddr>>,
}

impl WebRtcTransport {
    /// Create a WebRTC custom transport, panicking if `config` is invalid.
    pub fn new(config: WebRtcTransportConfig) -> Self {
        Self::try_new(config).expect("invalid WebRTC transport configuration")
    }

    /// Create a WebRTC custom transport after validating `config`.
    pub fn try_new(config: WebRtcTransportConfig) -> Result<Self> {
        config.queues.validate()?;
        config.frame.validate()?;
        tracing::trace!(
            target: "iroh_webrtc_transport::transport",
            local_addr = ?config.local_addr,
            recv_capacity = config.queues.recv_capacity,
            send_capacity = config.queues.send_capacity,
            max_payload_len = config.frame.max_payload_len,
            max_transmit_segments = config.max_transmit_segments.get(),
            "creating WebRTC custom transport"
        );
        let hub =
            SessionHub::with_capacities(config.queues.recv_capacity, config.queues.send_capacity);
        let local_addrs =
            n0_watcher::Watchable::new(config.local_addr.clone().into_iter().collect());
        Ok(Self {
            config,
            hub,
            local_addrs,
        })
    }

    /// Return the shared session hub used by WebRTC session pumps.
    pub fn session_hub(&self) -> SessionHub {
        self.hub.clone()
    }

    /// Return the currently advertised local custom transport addresses.
    pub fn local_addrs(&self) -> Vec<iroh_base::CustomAddr> {
        self.local_addrs.get()
    }

    /// Add one local custom transport address to the endpoint advertisement set.
    pub fn advertise_local_addr(&self, addr: iroh_base::CustomAddr) {
        let mut local_addrs = self.local_addrs.get();
        if local_addrs.iter().any(|existing| existing == &addr) {
            tracing::debug!(
                target: "iroh_webrtc_transport::transport",
                local_addr = ?addr,
                local_addrs = ?local_addrs,
                "WebRTC custom transport local address already advertised"
            );
            return;
        }
        local_addrs.push(addr.clone());
        let updated_addrs = local_addrs.clone();
        let _ = self.local_addrs.set(local_addrs);
        tracing::debug!(
            target: "iroh_webrtc_transport::transport",
            local_addr = ?addr,
            local_addrs = ?updated_addrs,
            "advertised WebRTC custom transport local address"
        );
    }

    /// Replace the complete local custom transport address advertisement set.
    pub fn set_local_addrs(&self, addrs: Vec<iroh_base::CustomAddr>) {
        let _ = self.local_addrs.set(addrs.clone());
        tracing::trace!(
            target: "iroh_webrtc_transport::transport",
            local_addrs = ?addrs,
            "set WebRTC custom transport local addresses"
        );
    }
}

impl CustomTransport for WebRtcTransport {
    fn bind(&self) -> io::Result<Box<dyn CustomEndpoint>> {
        tracing::trace!(
            target: "iroh_webrtc_transport::transport",
            local_addr = ?self.config.local_addr,
            "binding WebRTC custom transport endpoint"
        );
        Ok(Box::new(WebRtcEndpoint::new(
            self.local_addrs.clone(),
            self.hub.clone(),
            self.config.frame.max_payload_len,
            self.config.max_transmit_segments,
        )))
    }
}

/// Register a WebRTC custom transport with an Iroh endpoint builder.
pub fn configure_endpoint(builder: Builder, transport: WebRtcTransport) -> Builder {
    builder.add_custom_transport(Arc::new(transport))
}

impl Default for WebRtcTransportConfig {
    fn default() -> Self {
        Self {
            local_addr: None,
            queues: WebRtcQueueConfig::default(),
            frame: WebRtcFrameConfig::default(),
            max_transmit_segments: NonZeroUsize::new(10).expect("constant is nonzero"),
        }
    }
}
