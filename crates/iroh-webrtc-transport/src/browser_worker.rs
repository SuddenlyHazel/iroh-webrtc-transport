//! Worker-owned browser node foundations.
//!
//! This module intentionally keeps browser worker state in Rust without
//! exporting browser objects. It is the state machine a Wasm binding layer can
//! wrap while the main-thread integration owns `RTCPeerConnection` creation and
//! `RTCDataChannel` transfer.

use std::{
    cell::RefCell,
    collections::HashMap,
    fmt::Write as _,
    net::SocketAddr,
    str::FromStr,
    sync::{Arc, Mutex},
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, DynProtocolHandler, ProtocolHandler, Router},
};
use iroh_base::CustomAddr;
use serde_json::{Value, json};
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

pub(in crate::browser_worker) const DEFAULT_WORKER_ACCEPT_QUEUE_CAPACITY: usize = 32;
pub(in crate::browser_worker) use crate::browser_protocol::{
    BENCHMARK_ECHO_OPEN_COMMAND, BENCHMARK_LATENCY_COMMAND, BENCHMARK_THROUGHPUT_COMMAND,
    MAIN_RTC_ACCEPT_ANSWER_COMMAND, MAIN_RTC_ACCEPT_OFFER_COMMAND, MAIN_RTC_ADD_REMOTE_ICE_COMMAND,
    MAIN_RTC_CLOSE_PEER_CONNECTION_COMMAND, MAIN_RTC_CREATE_ANSWER_COMMAND,
    MAIN_RTC_CREATE_DATA_CHANNEL_COMMAND, MAIN_RTC_CREATE_OFFER_COMMAND,
    MAIN_RTC_CREATE_PEER_CONNECTION_COMMAND, MAIN_RTC_NEXT_LOCAL_ICE_COMMAND,
    MAIN_RTC_TAKE_DATA_CHANNEL_COMMAND, STREAM_ACCEPT_BI_COMMAND, STREAM_CANCEL_PENDING_COMMAND,
    STREAM_CLOSE_SEND_COMMAND, STREAM_OPEN_BI_COMMAND, STREAM_RECEIVE_CHUNK_COMMAND,
    STREAM_RESET_COMMAND, STREAM_SEND_CHUNK_COMMAND, WORKER_ACCEPT_CLOSE_COMMAND,
    WORKER_ACCEPT_NEXT_COMMAND, WORKER_ACCEPT_OPEN_COMMAND, WORKER_ATTACH_DATA_CHANNEL_COMMAND,
    WORKER_BOOTSTRAP_SIGNAL_COMMAND, WORKER_CONNECTION_CLOSE_COMMAND, WORKER_DIAL_COMMAND,
    WORKER_MAIN_RTC_RESULT_COMMAND, WORKER_NODE_CLOSE_COMMAND, WORKER_PROTOCOL_COMMAND_COMMAND,
    WORKER_PROTOCOL_NEXT_EVENT_COMMAND, WORKER_SPAWN_COMMAND,
};
const DEFAULT_STREAM_RECEIVE_CHUNK_BYTES: usize = 64 * 1024;

mod error;
mod main_rtc;
mod node;
mod protocol;
mod protocol_registry;
mod runtime;
mod types;

pub(in crate::browser_worker) use error::{
    BrowserWorkerError, BrowserWorkerErrorCode, BrowserWorkerResult, BrowserWorkerWireError,
};
#[cfg(any(test, all(target_family = "wasm", target_os = "unknown")))]
use main_rtc::PendingMainRtcRequest;
pub(in crate::browser_worker) use main_rtc::{
    WorkerMainAcceptAnswerPayload, WorkerMainAcceptOfferPayload, WorkerMainAddRemoteIcePayload,
    WorkerMainClosePeerConnectionPayload, WorkerMainCreateDataChannelPayload,
    WorkerMainCreatePeerConnectionPayload, WorkerMainRtcCommand, WorkerMainRtcResult,
    WorkerMainRtcResultEvent, WorkerMainRtcResultInput, WorkerMainSessionPayload,
    WorkerProtocolIceCandidate,
};
pub(in crate::browser_worker) use node::BrowserWorkerNode;
use protocol::*;
pub use protocol_registry::BrowserWorkerProtocolRegistry;
pub(in crate::browser_worker) use runtime::BrowserWorkerRuntimeCore;
#[cfg(test)]
pub(in crate::browser_worker) use types::BrowserWorkerNodeState;
pub(in crate::browser_worker) use types::{
    BrowserWorkerNodeConfig, WorkerAcceptId, WorkerAcceptNext, WorkerAcceptNextResult,
    WorkerAcceptOpenResult, WorkerAcceptRegistration, WorkerAcceptedConnection,
    WorkerAttachDataChannelResult, WorkerBootstrapSignalInput, WorkerBootstrapSignalResult,
    WorkerCloseResult, WorkerConnectionKey, WorkerDataChannelAttachmentState,
    WorkerDataChannelSource, WorkerDialAllocation, WorkerDialStartResult, WorkerNodeKey,
    WorkerProtocolConnectionInfo, WorkerProtocolTransportPrepareRequest, WorkerResolvedTransport,
    WorkerSessionKey, WorkerSessionLifecycle, WorkerSessionRole, WorkerSessionSnapshot,
    WorkerSpawnResult, WorkerStreamAcceptResult, WorkerStreamBackpressureState,
    WorkerStreamCancelPendingResult, WorkerStreamCloseSendResult, WorkerStreamKey,
    WorkerStreamOpenResult, WorkerStreamReceiveResult, WorkerStreamResetResult,
    WorkerStreamSendResult, WorkerTerminalDecision,
};

#[cfg(all(
    feature = "browser-worker",
    target_family = "wasm",
    target_os = "unknown"
))]
mod wasm_bindings;
#[cfg(all(
    feature = "browser-worker",
    target_family = "wasm",
    target_os = "unknown"
))]
pub use wasm_bindings::{start_browser_worker, start_browser_worker_with_protocols};

#[cfg(test)]
mod tests;
