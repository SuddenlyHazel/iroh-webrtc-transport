use iroh::EndpointId;

use crate::{
    browser_runtime::BrowserSessionKey,
    core::signaling::{BootstrapTransportIntent, DialId, WebRtcTerminalReason},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum BrowserResolvedTransport {
    #[serde(rename = "webrtc")]
    WebRtc,
    #[serde(rename = "irohRelay")]
    IrohRelay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) enum BrowserSessionRole {
    Dialer,
    Acceptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) enum DataChannelAttachmentState {
    NotRequired,
    AwaitingTransfer,
    Transferred,
    Open,
    Failed,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) enum BrowserSessionLifecycle {
    Allocated,
    WebRtcNegotiating,
    RelaySelected,
    ApplicationReady,
    Failed,
    Closed,
}

#[derive(Debug, Clone)]
pub(in crate::browser_runtime) struct BrowserDialAllocation {
    pub(in crate::browser_runtime) session_key: BrowserSessionKey,
    pub(in crate::browser_runtime) dial_id: DialId,
    pub(in crate::browser_runtime) generation: u64,
    pub(in crate::browser_runtime) local: EndpointId,
    pub(in crate::browser_runtime) remote: EndpointId,
    pub(in crate::browser_runtime) alpn: String,
    pub(in crate::browser_runtime) transport_intent: BootstrapTransportIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) struct BrowserDialStart {
    pub(in crate::browser_runtime) session_key: String,
    pub(in crate::browser_runtime) dial_id: String,
    pub(in crate::browser_runtime) generation: u64,
    pub(in crate::browser_runtime) remote_endpoint: String,
    pub(in crate::browser_runtime) alpn: String,
    pub(in crate::browser_runtime) transport_intent: BootstrapTransportIntent,
    pub(in crate::browser_runtime) bootstrap_signal: crate::core::signaling::WebRtcSignal,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) struct BrowserSessionSnapshot {
    pub(in crate::browser_runtime) session_key: BrowserSessionKey,
    pub(in crate::browser_runtime) dial_id: DialId,
    pub(in crate::browser_runtime) generation: u64,
    pub(in crate::browser_runtime) local: EndpointId,
    pub(in crate::browser_runtime) remote: EndpointId,
    pub(in crate::browser_runtime) alpn: String,
    pub(in crate::browser_runtime) role: BrowserSessionRole,
    pub(in crate::browser_runtime) transport_intent: BootstrapTransportIntent,
    pub(in crate::browser_runtime) resolved_transport: Option<BrowserResolvedTransport>,
    pub(in crate::browser_runtime) channel_attachment: DataChannelAttachmentState,
    pub(in crate::browser_runtime) lifecycle: BrowserSessionLifecycle,
    pub(in crate::browser_runtime) terminal_sent: Option<WebRtcTerminalReason>,
    pub(in crate::browser_runtime) terminal_received: Option<WebRtcTerminalReason>,
}
