//! Browser runtime-owned node foundations.
//!
//! This module intentionally keeps browser runtime state in Rust without
//! exporting browser objects. It is the state machine a Wasm binding layer can
//! wrap while the main-thread integration owns `RTCPeerConnection` creation and
//! `RTCDataChannel` transfer.

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
use std::cell::RefCell;
use std::{
    collections::HashMap,
    fmt::Write as _,
    sync::{Arc, Mutex},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, DynProtocolHandler, ProtocolHandler, Router},
};
use iroh_base::CustomAddr;
use tokio::sync::{mpsc, oneshot};

use crate::core::hub::SessionHub;
use crate::{
    config::WebRtcSessionConfig,
    core::{
        addr::WebRtcAddr,
        bootstrap::WEBRTC_BOOTSTRAP_ALPN,
        coordinator::DialCoordinator,
        signaling::{
            BootstrapTransportIntent, DialId, DialIdGenerator, DialSessionId, WebRtcSignal,
            WebRtcTerminalReason,
        },
    },
    transport::WebRtcTransport,
};

pub(in crate::browser_runtime) const DEFAULT_BROWSER_ACCEPT_QUEUE_CAPACITY: usize = 32;
const DEFAULT_STREAM_RECEIVE_CHUNK_BYTES: usize = 64 * 1024;

mod error;
mod node;
mod protocol;
mod protocol_registry;
mod rtc_lifecycle;
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
mod runtime;
mod types;

pub(in crate::browser_runtime) use error::{
    BrowserRuntimeError, BrowserRuntimeErrorCode, BrowserRuntimeResult,
};
pub(in crate::browser_runtime) use node::BrowserRuntimeNode;
use protocol::*;
pub use protocol_registry::BrowserProtocolRegistry;
pub(in crate::browser_runtime) use rtc_lifecycle::{
    RtcLifecycleEvent, RtcLifecycleInput, RtcLifecycleResult,
};
#[cfg(all(target_family = "wasm", target_os = "unknown"))]
pub(in crate::browser_runtime) use runtime::BrowserRuntimeCore;
#[cfg(test)]
pub(in crate::browser_runtime) use types::BrowserRuntimeNodeState;
pub(in crate::browser_runtime) use types::{
    AttachDataChannelOutcome, BrowserAcceptId, BrowserAcceptNext, BrowserAcceptRegistration,
    BrowserAcceptedConnection, BrowserBootstrapSignalInput, BrowserBootstrapSignalResult,
    BrowserConnectionKey, BrowserDialAllocation, BrowserDialStart, BrowserResolvedTransport,
    BrowserRuntimeNodeConfig, BrowserSessionKey, BrowserSessionLifecycle, BrowserSessionRole,
    BrowserSessionSnapshot, BrowserStreamBackpressureState, BrowserStreamKey,
    BrowserTerminalDecision, DataChannelAttachmentState, DataChannelSource,
    ProtocolTransportPrepareRequest,
};
pub(crate) use types::{
    BrowserAcceptNextInfo, BrowserAcceptRegistrationInfo, BrowserCloseOutcome,
    BrowserConnectionInfo, BrowserNodeInfo, BrowserStreamAcceptInfo, BrowserStreamCloseSendOutcome,
    BrowserStreamInfo, BrowserStreamReceiveOutcome, BrowserStreamSendOutcome,
};

#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
mod direct;
#[cfg(all(
    feature = "browser-main-thread",
    target_family = "wasm",
    target_os = "unknown"
))]
pub(crate) use direct::{
    BrowserRuntime, BrowserRuntimeLatencyStats, BrowserRuntimeThroughputStats,
};

#[cfg(test)]
mod tests;
