use std::fmt::Write as _;

use crate::{
    browser_runtime::{BrowserRuntimeError, BrowserRuntimeErrorCode, BrowserRuntimeResult},
    core::signaling::DialId,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct BrowserSessionKey(pub(in crate::browser_runtime) String);

impl BrowserSessionKey {
    pub(in crate::browser_runtime) fn new(value: impl Into<String>) -> BrowserRuntimeResult<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                "session key must not be empty",
            ));
        }
        Ok(Self(value))
    }

    pub(in crate::browser_runtime) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<DialId> for BrowserSessionKey {
    fn from(value: DialId) -> Self {
        let mut key = String::with_capacity(value.0.len() * 2);
        for byte in value.0 {
            write!(&mut key, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self(key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct BrowserAcceptId(pub(in crate::browser_runtime) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct BrowserConnectionKey(pub(in crate::browser_runtime) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct BrowserStreamKey(pub(in crate::browser_runtime) u64);
