use std::future::Future;

use serde::{Serialize, de::DeserializeOwned};

/// Typed worker-owned browser protocol contract.
pub trait BrowserWorkerProtocol: Clone + std::fmt::Debug + Send + Sync + 'static {
    const ALPN: &'static [u8];

    type Command: Serialize + DeserializeOwned + 'static;
    type Event: Serialize + DeserializeOwned + 'static;
    type Handler: iroh::protocol::ProtocolHandler + Clone + 'static;

    fn handler(&self, endpoint: iroh::Endpoint) -> Self::Handler;

    fn handle_command(
        &self,
        command: Self::Command,
    ) -> impl Future<Output = crate::Result<()>> + Send;

    fn next_event(&self) -> impl Future<Output = crate::Result<Option<Self::Event>>> + Send;
}
