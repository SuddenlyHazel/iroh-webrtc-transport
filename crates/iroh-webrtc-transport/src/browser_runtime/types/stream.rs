#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserStreamInfo {
    pub(crate) stream_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserStreamAcceptInfo {
    pub(crate) done: bool,
    pub(crate) stream_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserStreamSendOutcome {
    pub(crate) accepted_bytes: usize,
    pub(crate) buffered_bytes: usize,
    pub(crate) backpressure: BrowserStreamBackpressureState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum BrowserStreamBackpressureState {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserStreamReceiveOutcome {
    pub(crate) done: bool,
    pub(crate) chunk: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserStreamCloseSendOutcome {
    pub(crate) closed: bool,
}
