#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerStreamOpenResult {
    pub(in crate::browser_worker) stream_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerStreamAcceptResult {
    pub(in crate::browser_worker) done: bool,
    pub(in crate::browser_worker) stream_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerStreamSendResult {
    pub(in crate::browser_worker) accepted_bytes: usize,
    pub(in crate::browser_worker) buffered_bytes: usize,
    pub(in crate::browser_worker) backpressure: WorkerStreamBackpressureState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) enum WorkerStreamBackpressureState {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerStreamReceiveResult {
    pub(in crate::browser_worker) done: bool,
    pub(in crate::browser_worker) chunk: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerStreamCloseSendResult {
    pub(in crate::browser_worker) closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerStreamResetResult {
    pub(in crate::browser_worker) reset: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerStreamCancelPendingResult {
    pub(in crate::browser_worker) request_id: u64,
    pub(in crate::browser_worker) cancelled: bool,
}
