use std::{fmt, rc::Rc};

use super::{registry::WebRtcMainThreadRegistry, worker_bridge::WebRtcMainThreadWorkerBridge};
use crate::{
    browser::{
        BENCHMARK_ECHO_OPEN_COMMAND, BENCHMARK_LATENCY_COMMAND, BENCHMARK_THROUGHPUT_COMMAND,
        STREAM_SEND_CHUNK_COMMAND,
        js_wire::{js_array_buffer_like_to_vec, js_context_error, set_js_value},
    },
    config::WebRtcIceConfig,
};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Worker, WorkerOptions, WorkerType};

const DEFAULT_WORKER_FACTORY_GLOBAL: &str = "createIrohWebRtcWorker";

/// Browser app facade configuration.
#[derive(Clone)]
pub struct BrowserWebRtcNodeConfig {
    /// URL of the JavaScript module worker that hosts the browser worker Wasm runtime.
    pub worker_url: String,
    /// Optional browser-provided worker factory.
    ///
    /// When set, this is called with no arguments and must return a `Worker`.
    pub worker_factory: Option<js_sys::Function>,
    /// STUN server URLs used for direct WebRTC negotiation.
    pub stun_urls: Vec<String>,
    /// Request low-latency QUIC ACK behavior for browser WebRTC-backed Iroh endpoints.
    pub low_latency_quic_acks: bool,
}

impl Default for BrowserWebRtcNodeConfig {
    fn default() -> Self {
        Self {
            worker_url: "./iroh-webrtc-worker.js".into(),
            worker_factory: None,
            stun_urls: WebRtcIceConfig::default_direct_stun().stun_urls,
            low_latency_quic_acks: false,
        }
    }
}

impl fmt::Debug for BrowserWebRtcNodeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserWebRtcNodeConfig")
            .field("worker_url", &self.worker_url)
            .field("worker_factory", &self.worker_factory.is_some())
            .field("stun_urls", &self.stun_urls)
            .field("low_latency_quic_acks", &self.low_latency_quic_acks)
            .finish()
    }
}

impl PartialEq for BrowserWebRtcNodeConfig {
    fn eq(&self, other: &Self) -> bool {
        self.worker_url == other.worker_url
            && worker_factory_eq(&self.worker_factory, &other.worker_factory)
            && self.stun_urls == other.stun_urls
            && self.low_latency_quic_acks == other.low_latency_quic_acks
    }
}

impl Eq for BrowserWebRtcNodeConfig {}

impl BrowserWebRtcNodeConfig {
    pub fn with_worker_url(mut self, worker_url: impl Into<String>) -> Self {
        self.worker_url = worker_url.into();
        self.worker_factory = None;
        self
    }

    pub fn with_worker_factory(mut self, worker_factory: js_sys::Function) -> Self {
        self.worker_factory = Some(worker_factory);
        self
    }

    pub fn with_low_latency_quic_acks(mut self, enabled: bool) -> Self {
        self.low_latency_quic_acks = enabled;
        self
    }
}

/// Preferred transport behavior for browser dials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserDialTransportPreference {
    IrohRelay,
    WebRtcPreferred,
    WebRtcOnly,
}

impl BrowserDialTransportPreference {
    fn as_wire_value(self) -> &'static str {
        match self {
            Self::IrohRelay => "irohRelay",
            Self::WebRtcPreferred => "webrtcPreferred",
            Self::WebRtcOnly => "webrtcOnly",
        }
    }
}

/// Browser app facade dial options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserDialOptions {
    pub transport_preference: BrowserDialTransportPreference,
}

impl BrowserDialOptions {
    pub fn iroh_relay() -> Self {
        Self {
            transport_preference: BrowserDialTransportPreference::IrohRelay,
        }
    }

    pub fn webrtc_preferred() -> Self {
        Self {
            transport_preference: BrowserDialTransportPreference::WebRtcPreferred,
        }
    }

    pub fn webrtc_only() -> Self {
        Self {
            transport_preference: BrowserDialTransportPreference::WebRtcOnly,
        }
    }
}

impl Default for BrowserDialOptions {
    fn default() -> Self {
        Self::webrtc_preferred()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserWorkerLatencyStats {
    pub samples: usize,
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserWorkerThroughputStats {
    pub bytes: usize,
    pub upload_ms: f64,
    pub download_ms: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerSpawnResult {
    #[serde(rename = "nodeKey")]
    _node_key: String,
    endpoint_id: String,
    #[serde(rename = "localCustomAddr")]
    _local_custom_addr: String,
    #[serde(rename = "bootstrapAlpn")]
    _bootstrap_alpn: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerAcceptOpenResult {
    accept_key: String,
    #[serde(rename = "alpn")]
    _alpn: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerAcceptNextResult {
    done: bool,
    connection: Option<WorkerConnectionResult>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerConnectionResult {
    connection_key: String,
    remote_endpoint: String,
    alpn: String,
    #[serde(rename = "transport")]
    _transport: String,
    #[serde(rename = "dialId")]
    _dial_id: Option<String>,
    #[serde(rename = "sessionKey")]
    _session_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerStreamOpenResult {
    stream_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkerStreamAcceptResult {
    done: bool,
    stream_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkerStreamReceiveResult {
    done: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerSpawnPayload {
    stun_urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    low_latency_quic_acks: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerDialPayload {
    remote_endpoint: String,
    alpn: String,
    transport_intent: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerBenchmarkLatencyPayload {
    remote_endpoint: String,
    alpn: String,
    transport_intent: String,
    samples: usize,
    warmups: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerBenchmarkThroughputPayload {
    remote_endpoint: String,
    alpn: String,
    transport_intent: String,
    bytes: usize,
    warmup_bytes: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerAlpnPayload {
    alpn: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerAcceptPayload {
    accept_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerAcceptReasonPayload {
    accept_key: String,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerConnectionPayload {
    connection_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerConnectionReasonPayload {
    connection_key: String,
    reason: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerStreamPayload {
    stream_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkerReasonPayload {
    reason: String,
}

#[derive(Clone)]
pub struct BrowserWebRtcNode {
    inner: Rc<BrowserWebRtcNodeInner>,
}

struct BrowserWebRtcNodeInner {
    endpoint_id: String,
    bridge: WebRtcMainThreadWorkerBridge,
}

impl BrowserWebRtcNode {
    pub async fn spawn(config: BrowserWebRtcNodeConfig) -> Result<Self, JsValue> {
        let worker = create_worker(&config)?;
        let registry = WebRtcMainThreadRegistry::new();
        let bridge = WebRtcMainThreadWorkerBridge::new(worker, &registry)?;
        bridge.attach_rtc_control_channel()?;

        let result: WorkerSpawnResult =
            await_worker_result(bridge.spawn(spawn_payload(&config)?)?).await?;
        let endpoint_id = result.endpoint_id;

        Ok(Self {
            inner: Rc::new(BrowserWebRtcNodeInner {
                endpoint_id,
                bridge,
            }),
        })
    }

    pub fn endpoint_id(&self) -> &str {
        &self.inner.endpoint_id
    }

    pub async fn dial(
        &self,
        remote_endpoint: &str,
        alpn: &[u8],
        options: BrowserDialOptions,
    ) -> Result<BrowserWebRtcConnection, JsValue> {
        let result = await_worker_result(self.inner.bridge.dial(dial_payload(
            remote_endpoint,
            alpn,
            options,
        )?)?)
        .await?;
        Ok(BrowserWebRtcConnection::from_result(
            self.inner.clone(),
            result,
        ))
    }

    pub async fn accept(&self, alpn: &[u8]) -> Result<BrowserWebRtcAcceptor, JsValue> {
        let result: WorkerAcceptOpenResult =
            await_worker_result(self.inner.bridge.accept_open(alpn_payload(alpn)?)?).await?;
        Ok(BrowserWebRtcAcceptor {
            node: self.inner.clone(),
            accept_key: result.accept_key,
            closed: Rc::new(std::cell::Cell::new(false)),
        })
    }

    pub async fn open_worker_latency_echo(&self, alpn: &[u8]) -> Result<(), JsValue> {
        let _ = await_promise(self.inner.bridge.request(
            BENCHMARK_ECHO_OPEN_COMMAND.into(),
            alpn_payload(alpn)?,
            None,
        )?)
        .await?;
        Ok(())
    }

    pub async fn check_worker_latency(
        &self,
        remote_endpoint: &str,
        alpn: &[u8],
        samples: usize,
        warmups: usize,
    ) -> Result<BrowserWorkerLatencyStats, JsValue> {
        let result = await_worker_result(self.inner.bridge.request(
            BENCHMARK_LATENCY_COMMAND.into(),
            benchmark_latency_payload(remote_endpoint, alpn, samples, warmups)?,
            None,
        )?)
        .await?;
        Ok(result)
    }

    pub async fn check_worker_throughput(
        &self,
        remote_endpoint: &str,
        alpn: &[u8],
        bytes: usize,
        warmup_bytes: usize,
    ) -> Result<BrowserWorkerThroughputStats, JsValue> {
        let result: BrowserWorkerThroughputStats = await_worker_result(self.inner.bridge.request(
            BENCHMARK_THROUGHPUT_COMMAND.into(),
            benchmark_throughput_payload(remote_endpoint, alpn, bytes, warmup_bytes)?,
            None,
        )?)
        .await?;
        Ok(result)
    }

    pub async fn close(&self, reason: &str) -> Result<(), JsValue> {
        let _ = await_promise(self.inner.bridge.node_close(reason_payload(reason)?)?).await?;
        self.inner.bridge.close();
        Ok(())
    }
}

#[derive(Clone)]
pub struct BrowserWebRtcAcceptor {
    node: Rc<BrowserWebRtcNodeInner>,
    accept_key: String,
    closed: Rc<std::cell::Cell<bool>>,
}

impl BrowserWebRtcAcceptor {
    pub async fn accept(&self) -> Result<Option<BrowserWebRtcConnection>, JsValue> {
        let result: WorkerAcceptNextResult = await_worker_result(
            self.node
                .bridge
                .accept_next(accept_payload(&self.accept_key)?)?,
        )
        .await?;
        if result.done {
            self.closed.set(true);
            return Ok(None);
        }
        let connection = result
            .connection
            .ok_or_else(|| JsValue::from_str("browser worker accept result missing connection"))?;
        Ok(Some(BrowserWebRtcConnection::from_result(
            self.node.clone(),
            connection,
        )))
    }

    pub async fn close(&self, reason: &str) -> Result<(), JsValue> {
        if self.closed.replace(true) {
            return Ok(());
        }
        let payload = accept_reason_payload(&self.accept_key, reason)?;
        let _ = await_promise(self.node.bridge.accept_close(payload)?).await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct BrowserWebRtcConnection {
    node: Rc<BrowserWebRtcNodeInner>,
    connection_key: String,
    remote_endpoint_id: String,
    alpn: String,
}

impl BrowserWebRtcConnection {
    fn from_result(node: Rc<BrowserWebRtcNodeInner>, result: WorkerConnectionResult) -> Self {
        Self {
            node,
            connection_key: result.connection_key,
            remote_endpoint_id: result.remote_endpoint,
            alpn: result.alpn,
        }
    }

    pub fn remote_endpoint_id(&self) -> &str {
        &self.remote_endpoint_id
    }

    pub fn alpn(&self) -> &str {
        &self.alpn
    }

    pub async fn open_bi(&self) -> Result<BrowserWebRtcStream, JsValue> {
        let result: WorkerStreamOpenResult = await_worker_result(
            self.node
                .bridge
                .open_bi(connection_payload(&self.connection_key)?)?,
        )
        .await?;
        Ok(BrowserWebRtcStream::from_result(self.node.clone(), result))
    }

    pub async fn accept_bi(&self) -> Result<BrowserWebRtcStream, JsValue> {
        let result: WorkerStreamAcceptResult = await_worker_result(
            self.node
                .bridge
                .accept_bi(connection_payload(&self.connection_key)?)?,
        )
        .await?;
        if result.done {
            return Err(JsValue::from_str("connection stream accept completed"));
        }
        let stream_key = result.stream_key.ok_or_else(|| {
            JsValue::from_str("browser worker stream accept result missing streamKey")
        })?;
        Ok(BrowserWebRtcStream {
            node: self.node.clone(),
            stream_key,
        })
    }

    pub async fn close(&self, reason: &str) -> Result<(), JsValue> {
        let payload = connection_reason_payload(&self.connection_key, reason)?;
        let _ = await_promise(self.node.bridge.connection_close(payload)?).await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct BrowserWebRtcStream {
    node: Rc<BrowserWebRtcNodeInner>,
    stream_key: String,
}

impl BrowserWebRtcStream {
    fn from_result(node: Rc<BrowserWebRtcNodeInner>, result: WorkerStreamOpenResult) -> Self {
        Self {
            node,
            stream_key: result.stream_key,
        }
    }

    pub async fn send_all(&self, bytes: &[u8]) -> Result<(), JsValue> {
        let chunk = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
        chunk.copy_from(bytes);
        let buffer = chunk.buffer();
        let transfer = js_sys::Array::new();
        transfer.push(buffer.as_ref());
        let payload = stream_chunk_payload(&self.stream_key, buffer)?;
        let _ = await_promise(self.node.bridge.request(
            STREAM_SEND_CHUNK_COMMAND.into(),
            payload,
            Some(transfer),
        )?)
        .await?;
        Ok(())
    }

    pub async fn close_send(&self) -> Result<(), JsValue> {
        let _ = await_promise(
            self.node
                .bridge
                .close_send(stream_payload(&self.stream_key)?)?,
        )
        .await?;
        Ok(())
    }

    pub async fn read_to_end(&self) -> Result<Vec<u8>, JsValue> {
        let mut out = Vec::new();
        loop {
            let Some(chunk) = self.read_chunk().await? else {
                return Ok(out);
            };
            out.extend_from_slice(&chunk);
        }
    }

    pub async fn read_chunk(&self) -> Result<Option<Vec<u8>>, JsValue> {
        let result = await_promise(
            self.node
                .bridge
                .receive(stream_payload(&self.stream_key)?)?,
        )
        .await?;
        let meta: WorkerStreamReceiveResult = decode_worker_result(result.clone())?;
        if meta.done {
            return Ok(None);
        }
        let chunk = js_sys::Reflect::get(&result, &JsValue::from_str("chunk"))
            .map_err(|err| js_context_error("failed to read stream receive chunk", err))?;
        if chunk.is_undefined() || chunk.is_null() {
            return Ok(Some(Vec::new()));
        }
        js_array_buffer_like_to_vec(&chunk)
            .map(Some)
            .map_err(js_error)
    }
}

async fn await_promise(promise: js_sys::Promise) -> Result<JsValue, JsValue> {
    JsFuture::from(promise).await
}

async fn await_worker_result<T: for<'de> Deserialize<'de>>(
    promise: js_sys::Promise,
) -> Result<T, JsValue> {
    decode_worker_result(await_promise(promise).await?)
}

fn decode_worker_result<T: for<'de> Deserialize<'de>>(value: JsValue) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(value)
        .map_err(|err| JsValue::from_str(&format!("malformed browser worker result: {err}")))
}

fn create_worker(config: &BrowserWebRtcNodeConfig) -> Result<Worker, JsValue> {
    if let Some(factory) = config.worker_factory.as_ref() {
        return create_worker_from_factory(factory);
    }
    if let Some(factory) = global_worker_factory(DEFAULT_WORKER_FACTORY_GLOBAL)? {
        return create_worker_from_factory(&factory);
    }
    create_worker_from_url(&config.worker_url)
}

fn create_worker_from_factory(factory: &js_sys::Function) -> Result<Worker, JsValue> {
    let value = factory
        .call0(&JsValue::NULL)
        .map_err(|err| js_context_error("browser worker factory failed to create a Worker", err))?;
    value
        .dyn_into::<Worker>()
        .map_err(|_| JsValue::from_str("browser worker factory must return a Worker"))
}

fn global_worker_factory(name: &str) -> Result<Option<js_sys::Function>, JsValue> {
    let value = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(name))
        .map_err(|err| js_context_error("failed to read global browser worker factory", err))?;
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    value
        .dyn_into::<js_sys::Function>()
        .map(Some)
        .map_err(|_| JsValue::from_str("global browser worker factory must be a function"))
}

fn create_worker_from_url(worker_url: &str) -> Result<Worker, JsValue> {
    let options = WorkerOptions::new();
    options.set_type(WorkerType::Module);
    Worker::new_with_options(worker_url, &options)
}

fn worker_factory_eq(left: &Option<js_sys::Function>, right: &Option<js_sys::Function>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => js_sys::Object::is(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn spawn_payload(config: &BrowserWebRtcNodeConfig) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerSpawnPayload {
        stun_urls: config.stun_urls.clone(),
        low_latency_quic_acks: config.low_latency_quic_acks.then_some(true),
    })
}

fn dial_payload(
    remote_endpoint: &str,
    alpn: &[u8],
    options: BrowserDialOptions,
) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerDialPayload {
        remote_endpoint: remote_endpoint.to_owned(),
        alpn: String::from_utf8_lossy(alpn).into_owned(),
        transport_intent: options.transport_preference.as_wire_value().to_owned(),
    })
}

fn benchmark_latency_payload(
    remote_endpoint: &str,
    alpn: &[u8],
    samples: usize,
    warmups: usize,
) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerBenchmarkLatencyPayload {
        remote_endpoint: remote_endpoint.to_owned(),
        alpn: String::from_utf8_lossy(alpn).into_owned(),
        transport_intent: BrowserDialTransportPreference::WebRtcOnly
            .as_wire_value()
            .to_owned(),
        samples,
        warmups,
    })
}

fn benchmark_throughput_payload(
    remote_endpoint: &str,
    alpn: &[u8],
    bytes: usize,
    warmup_bytes: usize,
) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerBenchmarkThroughputPayload {
        remote_endpoint: remote_endpoint.to_owned(),
        alpn: String::from_utf8_lossy(alpn).into_owned(),
        transport_intent: BrowserDialTransportPreference::WebRtcOnly
            .as_wire_value()
            .to_owned(),
        bytes,
        warmup_bytes,
    })
}

fn alpn_payload(alpn: &[u8]) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerAlpnPayload {
        alpn: String::from_utf8_lossy(alpn).into_owned(),
    })
}

fn accept_payload(accept_key: &str) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerAcceptPayload {
        accept_key: accept_key.to_owned(),
    })
}

fn accept_reason_payload(accept_key: &str, reason: &str) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerAcceptReasonPayload {
        accept_key: accept_key.to_owned(),
        reason: reason.to_owned(),
    })
}

fn connection_payload(connection_key: &str) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerConnectionPayload {
        connection_key: connection_key.to_owned(),
    })
}

fn connection_reason_payload(connection_key: &str, reason: &str) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerConnectionReasonPayload {
        connection_key: connection_key.to_owned(),
        reason: reason.to_owned(),
    })
}

fn stream_payload(stream_key: &str) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerStreamPayload {
        stream_key: stream_key.to_owned(),
    })
}

fn stream_chunk_payload(stream_key: &str, chunk: js_sys::ArrayBuffer) -> Result<JsValue, JsValue> {
    let payload = encode_payload(&WorkerStreamPayload {
        stream_key: stream_key.to_owned(),
    })?;
    let object = payload
        .clone()
        .dyn_into::<js_sys::Object>()
        .map_err(|_| JsValue::from_str("stream chunk payload must be an object"))?;
    set_js_value(&object, "chunk", chunk.into()).map_err(js_error)?;
    Ok(object.into())
}

fn reason_payload(reason: &str) -> Result<JsValue, JsValue> {
    encode_payload(&WorkerReasonPayload {
        reason: reason.to_owned(),
    })
}

fn encode_payload<T: Serialize>(payload: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(payload)
        .map_err(|err| JsValue::from_str(&format!("failed to encode worker payload: {err}")))
}

fn js_error(error: crate::error::Error) -> JsValue {
    JsValue::from_str(&error.to_string())
}
