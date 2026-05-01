use iroh::EndpointId;

use crate::browser_worker::{
    WorkerAcceptId, WorkerConnectionKey, WorkerResolvedTransport, WorkerSessionKey,
};

#[derive(Debug, Clone)]
pub(in crate::browser_worker) struct WorkerAcceptRegistration {
    pub(in crate::browser_worker) id: WorkerAcceptId,
    pub(in crate::browser_worker) alpn: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::browser_worker) struct WorkerAcceptedConnection {
    pub(in crate::browser_worker) key: WorkerConnectionKey,
    pub(in crate::browser_worker) session_key: Option<WorkerSessionKey>,
    pub(in crate::browser_worker) remote: EndpointId,
    pub(in crate::browser_worker) alpn: String,
    pub(in crate::browser_worker) transport: WorkerResolvedTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerProtocolConnectionInfo {
    pub(in crate::browser_worker) connection_key: String,
    pub(in crate::browser_worker) remote_endpoint: String,
    pub(in crate::browser_worker) alpn: String,
    pub(in crate::browser_worker) transport: WorkerResolvedTransport,
    pub(in crate::browser_worker) dial_id: Option<String>,
    pub(in crate::browser_worker) session_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerAcceptOpenResult {
    pub(in crate::browser_worker) accept_key: String,
    pub(in crate::browser_worker) alpn: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerAcceptNextResult {
    pub(in crate::browser_worker) done: bool,
    pub(in crate::browser_worker) connection: Option<WorkerProtocolConnectionInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::browser_worker) enum WorkerAcceptNext {
    Ready(WorkerAcceptedConnection),
    Done,
}
