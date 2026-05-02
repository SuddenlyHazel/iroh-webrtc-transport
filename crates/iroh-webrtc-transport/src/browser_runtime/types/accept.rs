use iroh::EndpointId;

use crate::browser_runtime::{
    BrowserAcceptId, BrowserConnectionKey, BrowserResolvedTransport, BrowserSessionKey,
};

#[derive(Debug, Clone)]
pub(in crate::browser_runtime) struct BrowserAcceptRegistration {
    pub(in crate::browser_runtime) id: BrowserAcceptId,
    pub(in crate::browser_runtime) alpn: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::browser_runtime) struct BrowserAcceptedConnection {
    pub(in crate::browser_runtime) key: BrowserConnectionKey,
    pub(in crate::browser_runtime) session_key: Option<BrowserSessionKey>,
    pub(in crate::browser_runtime) remote: EndpointId,
    pub(in crate::browser_runtime) alpn: String,
    pub(in crate::browser_runtime) transport: BrowserResolvedTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserConnectionInfo {
    pub(crate) connection_key: String,
    pub(crate) remote_endpoint: String,
    pub(crate) alpn: String,
    pub(crate) transport: BrowserResolvedTransport,
    pub(crate) dial_id: Option<String>,
    pub(crate) session_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserAcceptRegistrationInfo {
    pub(crate) accept_key: String,
    pub(crate) alpn: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserAcceptNextInfo {
    pub(crate) done: bool,
    pub(crate) connection: Option<BrowserConnectionInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::browser_runtime) enum BrowserAcceptNext {
    Ready(BrowserAcceptedConnection),
    Done,
}
