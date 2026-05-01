use crate::{
    browser_worker::{
        WorkerMainRtcCommand, WorkerProtocolConnectionInfo, WorkerResolvedTransport,
        WorkerSessionSnapshot,
    },
    core::signaling::{WebRtcSignal, WebRtcTerminalReason},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) enum WorkerDataChannelSource {
    Created,
    Received,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_worker) struct WorkerBootstrapSignalInput {
    pub(in crate::browser_worker) signal: WebRtcSignal,
    pub(in crate::browser_worker) alpn: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerBootstrapSignalResult {
    pub(in crate::browser_worker) accepted: bool,
    pub(in crate::browser_worker) session_key: Option<String>,
    pub(in crate::browser_worker) session: Option<WorkerSessionSnapshot>,
    pub(in crate::browser_worker) connection: Option<WorkerProtocolConnectionInfo>,
    pub(in crate::browser_worker) main_rtc: Vec<WorkerMainRtcCommand>,
    pub(in crate::browser_worker) outbound_signals: Vec<WebRtcSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerAttachDataChannelResult {
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) attached: bool,
    pub(in crate::browser_worker) connection_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerTerminalDecision {
    pub(in crate::browser_worker) session_key: String,
    pub(in crate::browser_worker) reason: WebRtcTerminalReason,
    pub(in crate::browser_worker) signal: Option<WebRtcSignal>,
    pub(in crate::browser_worker) selected_transport: Option<WorkerResolvedTransport>,
}
