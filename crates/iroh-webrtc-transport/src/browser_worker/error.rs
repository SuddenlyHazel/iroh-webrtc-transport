#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_worker) enum BrowserWorkerErrorCode {
    #[serde(rename = "spawnFailed")]
    SpawnFailed,
    #[serde(rename = "unsupportedBrowser")]
    UnsupportedBrowser,
    #[serde(rename = "invalidStunUrl")]
    InvalidStunUrl,
    #[serde(rename = "invalidRemote")]
    InvalidRemote,
    #[serde(rename = "unsupportedAlpn")]
    UnsupportedAlpn,
    #[serde(rename = "webrtcFailed")]
    WebRtcFailed,
    #[serde(rename = "dataChannelFailed")]
    DataChannelFailed,
    #[serde(rename = "bootstrapFailed")]
    BootstrapFailed,
    #[serde(rename = "closed")]
    Closed,
}

impl BrowserWorkerErrorCode {
    pub(in crate::browser_worker) fn as_str(self) -> &'static str {
        match self {
            Self::SpawnFailed => "spawnFailed",
            Self::UnsupportedBrowser => "unsupportedBrowser",
            Self::InvalidStunUrl => "invalidStunUrl",
            Self::InvalidRemote => "invalidRemote",
            Self::UnsupportedAlpn => "unsupportedAlpn",
            Self::WebRtcFailed => "webrtcFailed",
            Self::DataChannelFailed => "dataChannelFailed",
            Self::BootstrapFailed => "bootstrapFailed",
            Self::Closed => "closed",
        }
    }
}

impl std::fmt::Display for BrowserWorkerErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub(in crate::browser_worker) struct BrowserWorkerError {
    pub(in crate::browser_worker) code: BrowserWorkerErrorCode,
    pub(in crate::browser_worker) message: String,
}

impl BrowserWorkerError {
    pub(in crate::browser_worker) fn new(
        code: BrowserWorkerErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(in crate::browser_worker) fn closed() -> Self {
        Self::new(
            BrowserWorkerErrorCode::Closed,
            "browser worker node is closed",
        )
    }

    pub(in crate::browser_worker) fn wire_error(&self) -> BrowserWorkerWireError {
        BrowserWorkerWireError {
            code: self.code,
            message: self.message.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_worker) struct BrowserWorkerWireError {
    pub(in crate::browser_worker) code: BrowserWorkerErrorCode,
    pub(in crate::browser_worker) message: String,
}

pub(in crate::browser_worker) type BrowserWorkerResult<T> =
    std::result::Result<T, BrowserWorkerError>;
