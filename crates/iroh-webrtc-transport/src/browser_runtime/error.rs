#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_runtime) enum BrowserRuntimeErrorCode {
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

impl BrowserRuntimeErrorCode {
    pub(in crate::browser_runtime) fn as_str(self) -> &'static str {
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

impl std::fmt::Display for BrowserRuntimeErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub(in crate::browser_runtime) struct BrowserRuntimeError {
    pub(in crate::browser_runtime) code: BrowserRuntimeErrorCode,
    pub(in crate::browser_runtime) message: String,
}

impl BrowserRuntimeError {
    pub(in crate::browser_runtime) fn new(
        code: BrowserRuntimeErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub(in crate::browser_runtime) fn closed() -> Self {
        Self::new(
            BrowserRuntimeErrorCode::Closed,
            "browser runtime node is closed",
        )
    }
}

pub(in crate::browser_runtime) type BrowserRuntimeResult<T> =
    std::result::Result<T, BrowserRuntimeError>;
