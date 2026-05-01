use std::fmt::Write as _;

use crate::{
    browser_worker::{BrowserWorkerError, BrowserWorkerErrorCode, BrowserWorkerResult},
    core::signaling::DialId,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_worker) struct WorkerSessionKey(pub(in crate::browser_worker) String);

impl WorkerSessionKey {
    pub(in crate::browser_worker) fn new(value: impl Into<String>) -> BrowserWorkerResult<Self> {
        let value = value.into();
        if value.is_empty() {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "session key must not be empty",
            ));
        }
        Ok(Self(value))
    }

    pub(in crate::browser_worker) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<DialId> for WorkerSessionKey {
    fn from(value: DialId) -> Self {
        let mut key = String::with_capacity(value.0.len() * 2);
        for byte in value.0 {
            write!(&mut key, "{byte:02x}").expect("writing to String cannot fail");
        }
        Self(key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_worker) struct WorkerAcceptId(pub(in crate::browser_worker) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_worker) struct WorkerConnectionKey(pub(in crate::browser_worker) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_worker) struct WorkerStreamKey(pub(in crate::browser_worker) u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(in crate::browser_worker) struct WorkerNodeKey(pub(in crate::browser_worker) String);

impl WorkerNodeKey {
    pub(in crate::browser_worker) fn as_str(&self) -> &str {
        &self.0
    }
}
