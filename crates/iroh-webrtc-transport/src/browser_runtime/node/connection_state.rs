use iroh::endpoint::Connection;

use crate::browser_runtime::{BrowserResolvedTransport, BrowserSessionKey};

#[derive(Debug, Clone)]
pub(super) struct BrowserConnectionState {
    pub(super) iroh_connection: Option<Connection>,
    pub(super) session_key: Option<BrowserSessionKey>,
    pub(super) transport: BrowserResolvedTransport,
    pub(super) closed: bool,
}
