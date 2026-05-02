use super::*;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_runtime) struct RtcLifecycleResult {
    pub(in crate::browser_runtime) accepted: bool,
    pub(in crate::browser_runtime) session_key: String,
    pub(in crate::browser_runtime) session: Option<BrowserSessionSnapshot>,
    pub(in crate::browser_runtime) connection: Option<BrowserConnectionInfo>,
    pub(in crate::browser_runtime) outbound_signals: Vec<WebRtcSignal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::browser_runtime) struct RtcLifecycleInput {
    pub(in crate::browser_runtime) session_key: BrowserSessionKey,
    pub(in crate::browser_runtime) event: RtcLifecycleEvent,
    pub(in crate::browser_runtime) error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::browser_runtime) enum RtcLifecycleEvent {
    DataChannelOpen,
    WebRtcFailed {
        message: String,
    },
    #[cfg(all(target_arch = "wasm32", feature = "browser"))]
    WebRtcClosed {
        message: String,
    },
}
