use crate::{
    browser_runtime::{BrowserConnectionInfo, BrowserResolvedTransport, BrowserSessionSnapshot},
    core::signaling::{WebRtcSignal, WebRtcTerminalReason},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) enum DataChannelSource {
    Created,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_runtime) struct BrowserBootstrapSignalInput {
    pub(in crate::browser_runtime) signal: WebRtcSignal,
    pub(in crate::browser_runtime) alpn: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) struct BrowserBootstrapSignalResult {
    pub(in crate::browser_runtime) accepted: bool,
    pub(in crate::browser_runtime) session_key: Option<String>,
    pub(in crate::browser_runtime) session: Option<BrowserSessionSnapshot>,
    pub(in crate::browser_runtime) connection: Option<BrowserConnectionInfo>,
    pub(in crate::browser_runtime) outbound_signals: Vec<WebRtcSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) struct AttachDataChannelOutcome {
    pub(in crate::browser_runtime) session_key: String,
    pub(in crate::browser_runtime) attached: bool,
    pub(in crate::browser_runtime) connection_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) struct BrowserTerminalDecision {
    pub(in crate::browser_runtime) session_key: String,
    pub(in crate::browser_runtime) reason: WebRtcTerminalReason,
    pub(in crate::browser_runtime) signal: Option<WebRtcSignal>,
    pub(in crate::browser_runtime) selected_transport: Option<BrowserResolvedTransport>,
}
