#[cfg(test)]
use iroh::EndpointId;
#[cfg(test)]
use iroh_base::CustomAddr;
use tokio::sync::{mpsc, oneshot};

use crate::{
    browser_runtime::{
        BrowserRuntimeError, BrowserRuntimeErrorCode, BrowserRuntimeResult,
        DEFAULT_BROWSER_ACCEPT_QUEUE_CAPACITY,
    },
    config::WebRtcSessionConfig,
    core::signaling::{BootstrapTransportIntent, WebRtcSignal},
    transport::WebRtcTransportConfig,
};

#[derive(Debug, Clone)]
pub(in crate::browser_runtime) struct BrowserRuntimeNodeConfig {
    pub(in crate::browser_runtime) accept_queue_capacity: usize,
    pub(in crate::browser_runtime) session_config: WebRtcSessionConfig,
    pub(in crate::browser_runtime) transport_config: WebRtcTransportConfig,
    pub(in crate::browser_runtime) low_latency_quic_acks: bool,
    pub(in crate::browser_runtime) facade_alpns: Vec<String>,
    pub(in crate::browser_runtime) benchmark_echo_alpns: Vec<String>,
    pub(in crate::browser_runtime) protocol_transport_intent: BootstrapTransportIntent,
    pub(in crate::browser_runtime) protocol_transport_prepare_tx:
        Option<mpsc::Sender<ProtocolTransportPrepareRequest>>,
}

impl BrowserRuntimeNodeConfig {
    pub(in crate::browser_runtime) fn validate(&self) -> BrowserRuntimeResult<()> {
        if self.accept_queue_capacity == 0 {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::SpawnFailed,
                "accept queue capacity must be greater than zero",
            ));
        }
        self.session_config.validate().map_err(|err| {
            BrowserRuntimeError::new(BrowserRuntimeErrorCode::SpawnFailed, err.to_string())
        })?;
        self.transport_config.queues.validate().map_err(|err| {
            BrowserRuntimeError::new(BrowserRuntimeErrorCode::SpawnFailed, err.to_string())
        })?;
        self.transport_config.frame.validate().map_err(|err| {
            BrowserRuntimeError::new(BrowserRuntimeErrorCode::SpawnFailed, err.to_string())
        })?;
        Ok(())
    }
}

impl Default for BrowserRuntimeNodeConfig {
    fn default() -> Self {
        Self {
            accept_queue_capacity: DEFAULT_BROWSER_ACCEPT_QUEUE_CAPACITY,
            session_config: WebRtcSessionConfig::default(),
            transport_config: WebRtcTransportConfig::default(),
            low_latency_quic_acks: false,
            facade_alpns: Vec::new(),
            benchmark_echo_alpns: Vec::new(),
            protocol_transport_intent: BootstrapTransportIntent::WebRtcPreferred,
            protocol_transport_prepare_tx: None,
        }
    }
}

#[derive(Debug)]
pub(in crate::browser_runtime) struct ProtocolTransportPrepareRequest {
    pub(in crate::browser_runtime) remote: iroh::EndpointId,
    pub(in crate::browser_runtime) alpn: String,
    pub(in crate::browser_runtime) transport_intent: BootstrapTransportIntent,
    pub(in crate::browser_runtime) response: oneshot::Sender<BrowserRuntimeResult<()>>,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(in crate::browser_runtime) struct BrowserRuntimeNodeState {
    pub(in crate::browser_runtime) endpoint_id: EndpointId,
    pub(in crate::browser_runtime) local_custom_addr: CustomAddr,
    pub(in crate::browser_runtime) bootstrap_alpn: &'static str,
    pub(in crate::browser_runtime) closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserNodeInfo {
    pub(crate) node_key: String,
    pub(crate) endpoint_id: String,
    pub(crate) local_custom_addr: String,
    pub(crate) bootstrap_alpn: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserCloseOutcome {
    pub(crate) closed: bool,
    pub(crate) outbound_signals: Vec<WebRtcSignal>,
}
