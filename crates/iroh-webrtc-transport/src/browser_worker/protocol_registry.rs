use std::{any::TypeId, collections::HashMap, future::Future, pin::Pin, sync::Arc};

use iroh::protocol::DynProtocolHandler;
use serde_json::{Value, json};

use crate::{
    browser::BrowserWorkerProtocol,
    browser_worker::{BrowserWorkerError, BrowserWorkerErrorCode, BrowserWorkerResult},
};

#[derive(Clone, Default)]
pub struct BrowserWorkerProtocolRegistry {
    by_alpn: Arc<HashMap<String, Arc<dyn DynBrowserWorkerProtocol>>>,
    by_type: Arc<HashMap<TypeId, String>>,
}

impl BrowserWorkerProtocolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<P>(&mut self, protocol: P) -> Result<(), String>
    where
        P: BrowserWorkerProtocol,
    {
        let alpn = protocol_alpn_string(P::ALPN)?;
        let by_alpn = Arc::make_mut(&mut self.by_alpn);
        if by_alpn.contains_key(&alpn) {
            return Err(format!("duplicate worker protocol ALPN {alpn:?}"));
        }
        let by_type = Arc::make_mut(&mut self.by_type);
        if by_type.contains_key(&TypeId::of::<P>()) {
            return Err(format!(
                "worker protocol type {} was registered more than once",
                std::any::type_name::<P>()
            ));
        }
        by_alpn.insert(alpn.clone(), Arc::new(TypedBrowserWorkerProtocol(protocol)));
        by_type.insert(TypeId::of::<P>(), alpn);
        Ok(())
    }

    pub(in crate::browser_worker) fn contains_alpn(&self, alpn: &str) -> bool {
        self.by_alpn.contains_key(alpn)
    }

    pub(in crate::browser_worker) fn alpns(&self) -> impl Iterator<Item = &str> {
        self.by_alpn.keys().map(String::as_str)
    }

    pub(in crate::browser_worker) fn handler(
        &self,
        alpn: &str,
        endpoint: iroh::Endpoint,
    ) -> Option<Box<dyn DynProtocolHandler>> {
        self.by_alpn
            .get(alpn)
            .map(|protocol| protocol.handler(endpoint))
    }

    pub(in crate::browser_worker) async fn handle_command(
        &self,
        alpn: &str,
        command: Value,
    ) -> BrowserWorkerResult<Value> {
        let protocol = self.protocol(alpn)?;
        protocol.handle_command(command).await
    }

    pub(in crate::browser_worker) async fn next_event(
        &self,
        alpn: &str,
    ) -> BrowserWorkerResult<Value> {
        let protocol = self.protocol(alpn)?;
        protocol.next_event().await
    }

    fn protocol(&self, alpn: &str) -> BrowserWorkerResult<&dyn DynBrowserWorkerProtocol> {
        self.by_alpn
            .get(alpn)
            .map(|protocol| protocol.as_ref())
            .ok_or_else(|| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::UnsupportedAlpn,
                    format!("no worker-owned protocol registered for ALPN {alpn:?}"),
                )
            })
    }
}

trait DynBrowserWorkerProtocol: Send + Sync + std::fmt::Debug {
    fn handler(&self, endpoint: iroh::Endpoint) -> Box<dyn DynProtocolHandler>;

    fn handle_command(
        &self,
        command: Value,
    ) -> Pin<Box<dyn Future<Output = BrowserWorkerResult<Value>> + Send + '_>>;

    fn next_event(&self) -> Pin<Box<dyn Future<Output = BrowserWorkerResult<Value>> + Send + '_>>;
}

#[derive(Debug)]
struct TypedBrowserWorkerProtocol<P>(P);

impl<P> DynBrowserWorkerProtocol for TypedBrowserWorkerProtocol<P>
where
    P: BrowserWorkerProtocol,
{
    fn handler(&self, endpoint: iroh::Endpoint) -> Box<dyn DynProtocolHandler> {
        Box::new(self.0.handler(endpoint))
    }

    fn handle_command(
        &self,
        command: Value,
    ) -> Pin<Box<dyn Future<Output = BrowserWorkerResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let command = serde_json::from_value(command).map_err(|err| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    format!("malformed worker protocol command: {err}"),
                )
            })?;
            self.0
                .handle_command(command)
                .await
                .map_err(protocol_error)?;
            Ok(json!({ "handled": true }))
        })
    }

    fn next_event(&self) -> Pin<Box<dyn Future<Output = BrowserWorkerResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let event = self.0.next_event().await.map_err(protocol_error)?;
            serde_json::to_value(event).map_err(|err| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    format!("failed to encode worker protocol event: {err}"),
                )
            })
        })
    }
}

fn protocol_error(error: crate::Error) -> BrowserWorkerError {
    BrowserWorkerError::new(BrowserWorkerErrorCode::WebRtcFailed, error.to_string())
}

fn protocol_alpn_string(alpn: &[u8]) -> Result<String, String> {
    if alpn.is_empty() {
        return Err("worker protocol ALPN must not be empty".into());
    }
    std::str::from_utf8(alpn)
        .map(str::to_owned)
        .map_err(|err| format!("worker protocol ALPN must be UTF-8: {err}"))
}
