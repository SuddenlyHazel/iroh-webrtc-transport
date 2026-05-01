use iroh::endpoint::Connection;

use crate::browser_worker::{WorkerResolvedTransport, WorkerSessionKey};

#[derive(Debug, Clone)]
pub(super) struct WorkerConnectionState {
    pub(super) iroh_connection: Option<Connection>,
    pub(super) session_key: Option<WorkerSessionKey>,
    pub(super) transport: WorkerResolvedTransport,
    pub(super) closed: bool,
}
