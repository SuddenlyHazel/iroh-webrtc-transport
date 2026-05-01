use iroh::EndpointId;

use crate::{
    browser_worker::{WorkerMainRtcCommand, WorkerSessionKey},
    core::signaling::{BootstrapTransportIntent, DialId, WebRtcTerminalReason},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_worker) enum WorkerResolvedTransport {
    #[serde(rename = "webrtc")]
    WebRtc,
    #[serde(rename = "irohRelay")]
    IrohRelay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) enum WorkerSessionRole {
    Dialer,
    Acceptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) enum WorkerDataChannelAttachmentState {
    NotRequired,
    AwaitingTransfer,
    Transferred,
    Open,
    Failed,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) enum WorkerSessionLifecycle {
    Allocated,
    WebRtcNegotiating,
    RelaySelected,
    ApplicationReady,
    Failed,
    Closed,
}

#[derive(Debug, Clone)]
pub(in crate::browser_worker) struct WorkerDialAllocation {
    pub(in crate::browser_worker) session_key: WorkerSessionKey,
    pub(in crate::browser_worker) dial_id: DialId,
    pub(in crate::browser_worker) generation: u64,
    pub(in crate::browser_worker) local: EndpointId,
    pub(in crate::browser_worker) remote: EndpointId,
    pub(in crate::browser_worker) alpn: String,
    pub(in crate::browser_worker) transport_intent: BootstrapTransportIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerDialStartResult {
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) dial_id: String,
    pub(in crate::browser_worker) generation: u64,
    pub(in crate::browser_worker) remote_endpoint: String,
    pub(in crate::browser_worker) alpn: String,
    pub(in crate::browser_worker) transport_intent: BootstrapTransportIntent,
    pub(in crate::browser_worker) bootstrap_signal: crate::core::signaling::WebRtcSignal,
    pub(in crate::browser_worker) main_rtc: Vec<WorkerMainRtcCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerSessionSnapshot {
    pub(in crate::browser_worker) session_key: WorkerSessionKey,
    pub(in crate::browser_worker) dial_id: DialId,
    pub(in crate::browser_worker) generation: u64,
    pub(in crate::browser_worker) local: EndpointId,
    pub(in crate::browser_worker) remote: EndpointId,
    pub(in crate::browser_worker) alpn: String,
    pub(in crate::browser_worker) role: WorkerSessionRole,
    pub(in crate::browser_worker) transport_intent: BootstrapTransportIntent,
    pub(in crate::browser_worker) resolved_transport: Option<WorkerResolvedTransport>,
    pub(in crate::browser_worker) channel_attachment: WorkerDataChannelAttachmentState,
    pub(in crate::browser_worker) lifecycle: WorkerSessionLifecycle,
    pub(in crate::browser_worker) terminal_sent: Option<WebRtcTerminalReason>,
    pub(in crate::browser_worker) terminal_received: Option<WebRtcTerminalReason>,
}
