#[cfg(test)]
use iroh::EndpointId;
#[cfg(test)]
use iroh_base::CustomAddr;

use crate::{
    browser_worker::{
        BrowserWorkerError, BrowserWorkerErrorCode, BrowserWorkerResult,
        DEFAULT_WORKER_ACCEPT_QUEUE_CAPACITY,
    },
    config::WebRtcSessionConfig,
    core::signaling::WebRtcSignal,
    transport::WebRtcTransportConfig,
};

#[derive(Debug, Clone)]
pub(in crate::browser_worker) struct BrowserWorkerNodeConfig {
    pub(in crate::browser_worker) accept_queue_capacity: usize,
    pub(in crate::browser_worker) session_config: WebRtcSessionConfig,
    pub(in crate::browser_worker) transport_config: WebRtcTransportConfig,
    pub(in crate::browser_worker) low_latency_quic_acks: bool,
}

impl BrowserWorkerNodeConfig {
    pub(in crate::browser_worker) fn validate(&self) -> BrowserWorkerResult<()> {
        if self.accept_queue_capacity == 0 {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::SpawnFailed,
                "accept queue capacity must be greater than zero",
            ));
        }
        self.session_config.validate().map_err(|err| {
            BrowserWorkerError::new(BrowserWorkerErrorCode::SpawnFailed, err.to_string())
        })?;
        self.transport_config.queues.validate().map_err(|err| {
            BrowserWorkerError::new(BrowserWorkerErrorCode::SpawnFailed, err.to_string())
        })?;
        self.transport_config.frame.validate().map_err(|err| {
            BrowserWorkerError::new(BrowserWorkerErrorCode::SpawnFailed, err.to_string())
        })?;
        Ok(())
    }
}

impl Default for BrowserWorkerNodeConfig {
    fn default() -> Self {
        Self {
            accept_queue_capacity: DEFAULT_WORKER_ACCEPT_QUEUE_CAPACITY,
            session_config: WebRtcSessionConfig::default(),
            transport_config: WebRtcTransportConfig::default(),
            low_latency_quic_acks: false,
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(in crate::browser_worker) struct BrowserWorkerNodeState {
    pub(in crate::browser_worker) endpoint_id: EndpointId,
    pub(in crate::browser_worker) local_custom_addr: CustomAddr,
    pub(in crate::browser_worker) bootstrap_alpn: &'static str,
    pub(in crate::browser_worker) closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerSpawnResult {
    pub(in crate::browser_worker) node_key: String,
    pub(in crate::browser_worker) endpoint_id: String,
    pub(in crate::browser_worker) local_custom_addr: String,
    pub(in crate::browser_worker) bootstrap_alpn: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerCloseResult {
    pub(in crate::browser_worker) closed: bool,
    pub(in crate::browser_worker) outbound_signals: Vec<WebRtcSignal>,
}
