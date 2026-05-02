use std::{any::TypeId, collections::HashMap, future::Future, pin::Pin, sync::Arc};

use iroh::protocol::DynProtocolHandler;
use serde_json::{Value, json};

use crate::{
    browser::BrowserProtocol,
    browser_runtime::{BrowserRuntimeError, BrowserRuntimeErrorCode, BrowserRuntimeResult},
};

#[derive(Clone, Default)]
pub struct BrowserProtocolRegistry {
    by_alpn: Arc<HashMap<String, Arc<dyn DynBrowserProtocol>>>,
    by_type: Arc<HashMap<TypeId, String>>,
}

impl BrowserProtocolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<P>(&mut self, protocol: P) -> Result<(), String>
    where
        P: BrowserProtocol,
    {
        let alpn = protocol_alpn_string(P::ALPN)?;
        let by_alpn = Arc::make_mut(&mut self.by_alpn);
        if by_alpn.contains_key(&alpn) {
            return Err(format!("duplicate browser protocol ALPN {alpn:?}"));
        }
        let by_type = Arc::make_mut(&mut self.by_type);
        if by_type.contains_key(&TypeId::of::<P>()) {
            return Err(format!(
                "browser protocol type {} was registered more than once",
                std::any::type_name::<P>()
            ));
        }
        by_alpn.insert(alpn.clone(), Arc::new(TypedBrowserProtocol(protocol)));
        by_type.insert(TypeId::of::<P>(), alpn);
        Ok(())
    }

    pub(in crate::browser_runtime) fn contains_alpn(&self, alpn: &str) -> bool {
        self.by_alpn.contains_key(alpn)
    }

    pub(in crate::browser_runtime) fn alpns(&self) -> impl Iterator<Item = &str> {
        self.by_alpn.keys().map(String::as_str)
    }

    pub(in crate::browser_runtime) fn handler(
        &self,
        alpn: &str,
        endpoint: iroh::Endpoint,
    ) -> Option<Box<dyn DynProtocolHandler>> {
        self.by_alpn
            .get(alpn)
            .map(|protocol| protocol.handler(endpoint))
    }

    pub(in crate::browser_runtime) async fn handle_command(
        &self,
        alpn: &str,
        command: Value,
    ) -> BrowserRuntimeResult<Value> {
        let protocol = self.protocol(alpn)?;
        protocol.handle_command(command).await
    }

    pub(in crate::browser_runtime) async fn next_event(
        &self,
        alpn: &str,
    ) -> BrowserRuntimeResult<Value> {
        let protocol = self.protocol(alpn)?;
        protocol.next_event().await
    }

    fn protocol(&self, alpn: &str) -> BrowserRuntimeResult<&dyn DynBrowserProtocol> {
        self.by_alpn
            .get(alpn)
            .map(|protocol| protocol.as_ref())
            .ok_or_else(|| {
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::UnsupportedAlpn,
                    format!("no browser protocol registered for ALPN {alpn:?}"),
                )
            })
    }
}

trait DynBrowserProtocol: Send + Sync + std::fmt::Debug {
    fn handler(&self, endpoint: iroh::Endpoint) -> Box<dyn DynProtocolHandler>;

    fn handle_command(
        &self,
        command: Value,
    ) -> Pin<Box<dyn Future<Output = BrowserRuntimeResult<Value>> + Send + '_>>;

    fn next_event(&self) -> Pin<Box<dyn Future<Output = BrowserRuntimeResult<Value>> + Send + '_>>;
}

#[derive(Debug)]
struct TypedBrowserProtocol<P>(P);

impl<P> DynBrowserProtocol for TypedBrowserProtocol<P>
where
    P: BrowserProtocol,
{
    fn handler(&self, endpoint: iroh::Endpoint) -> Box<dyn DynProtocolHandler> {
        Box::new(self.0.handler(endpoint))
    }

    fn handle_command(
        &self,
        command: Value,
    ) -> Pin<Box<dyn Future<Output = BrowserRuntimeResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let command = serde_json::from_value(command).map_err(|err| {
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    format!("malformed browser protocol command: {err}"),
                )
            })?;
            self.0
                .handle_command(command)
                .await
                .map_err(protocol_error)?;
            Ok(json!({ "handled": true }))
        })
    }

    fn next_event(&self) -> Pin<Box<dyn Future<Output = BrowserRuntimeResult<Value>> + Send + '_>> {
        Box::pin(async move {
            let event = self.0.next_event().await.map_err(protocol_error)?;
            serde_json::to_value(event).map_err(|err| {
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    format!("failed to encode browser protocol event: {err}"),
                )
            })
        })
    }
}

fn protocol_error(error: crate::Error) -> BrowserRuntimeError {
    BrowserRuntimeError::new(BrowserRuntimeErrorCode::WebRtcFailed, error.to_string())
}

fn protocol_alpn_string(alpn: &[u8]) -> Result<String, String> {
    if alpn.is_empty() {
        return Err("browser protocol ALPN must not be empty".into());
    }
    std::str::from_utf8(alpn)
        .map(str::to_owned)
        .map_err(|err| format!("browser protocol ALPN must be UTF-8: {err}"))
}
