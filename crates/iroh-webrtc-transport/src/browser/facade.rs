use std::{fmt, rc::Rc};

use super::{BrowserRuntime, registry::BrowserRtcRegistry};
use crate::{
    browser::{BrowserProtocol, BrowserProtocolRegistry},
    browser_runtime::{
        BrowserAcceptNextInfo, BrowserAcceptRegistrationInfo, BrowserConnectionInfo,
        BrowserNodeInfo, BrowserRuntimeLatencyStats, BrowserRuntimeThroughputStats,
        BrowserStreamAcceptInfo, BrowserStreamInfo,
    },
    config::WebRtcIceConfig,
    core::signaling::BootstrapTransportIntent,
};
use iroh::{EndpointAddr, EndpointId};
use std::str::FromStr as _;
use wasm_bindgen::JsValue;

/// Preferred transport behavior for browser dials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserDialTransportPreference {
    IrohRelay,
    WebRtcPreferred,
    WebRtcOnly,
}

impl BrowserDialTransportPreference {
    fn as_intent(self) -> BootstrapTransportIntent {
        match self {
            Self::IrohRelay => BootstrapTransportIntent::IrohRelay,
            Self::WebRtcPreferred => BootstrapTransportIntent::WebRtcPreferred,
            Self::WebRtcOnly => BootstrapTransportIntent::WebRtcOnly,
        }
    }
}

/// Browser app facade configuration.
#[derive(Clone)]
pub struct BrowserWebRtcNodeConfig {
    /// STUN server URLs used for direct WebRTC negotiation.
    pub stun_urls: Vec<String>,
    /// Request low-latency QUIC ACK behavior for browser WebRTC-backed Iroh endpoints.
    pub low_latency_quic_acks: bool,
    /// Transport policy applied to browser-owned protocols that call `Endpoint::connect` directly.
    pub protocol_transport_preference: BrowserDialTransportPreference,
}

impl Default for BrowserWebRtcNodeConfig {
    fn default() -> Self {
        Self {
            stun_urls: WebRtcIceConfig::default_direct_stun().stun_urls,
            low_latency_quic_acks: false,
            protocol_transport_preference: BrowserDialTransportPreference::WebRtcPreferred,
        }
    }
}

impl fmt::Debug for BrowserWebRtcNodeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrowserWebRtcNodeConfig")
            .field("stun_urls", &self.stun_urls)
            .field("low_latency_quic_acks", &self.low_latency_quic_acks)
            .field(
                "protocol_transport_preference",
                &self.protocol_transport_preference,
            )
            .finish()
    }
}

impl PartialEq for BrowserWebRtcNodeConfig {
    fn eq(&self, other: &Self) -> bool {
        self.stun_urls == other.stun_urls
            && self.low_latency_quic_acks == other.low_latency_quic_acks
            && self.protocol_transport_preference == other.protocol_transport_preference
    }
}

impl Eq for BrowserWebRtcNodeConfig {}

impl BrowserWebRtcNodeConfig {
    pub fn with_low_latency_quic_acks(mut self, enabled: bool) -> Self {
        self.low_latency_quic_acks = enabled;
        self
    }

    pub fn with_protocol_transport_preference(
        mut self,
        preference: BrowserDialTransportPreference,
    ) -> Self {
        self.protocol_transport_preference = preference;
        self
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrowserLatencyStats {
    pub samples: usize,
    pub avg_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub min_ms: f64,
    pub max_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrowserThroughputStats {
    pub bytes: usize,
    pub upload_ms: f64,
    pub download_ms: f64,
}

#[derive(Clone)]
pub struct BrowserWebRtcNode {
    inner: Rc<BrowserWebRtcNodeInner>,
}

struct BrowserWebRtcNodeInner {
    endpoint_id: String,
    runtime: BrowserRuntime,
}

pub struct BrowserWebRtcNodeBuilder {
    config: BrowserWebRtcNodeConfig,
    facade_alpns: Vec<String>,
    benchmark_echo_alpns: Vec<String>,
    protocols: BrowserProtocolRegistry,
}

impl BrowserWebRtcNodeBuilder {
    pub fn accept_facade(mut self, alpn: impl AsRef<[u8]>) -> Self {
        self.facade_alpns
            .push(String::from_utf8_lossy(alpn.as_ref()).into_owned());
        self
    }

    pub fn accept_latency_echo(mut self, alpn: impl AsRef<[u8]>) -> Self {
        self.benchmark_echo_alpns
            .push(String::from_utf8_lossy(alpn.as_ref()).into_owned());
        self
    }

    pub fn protocol<P>(mut self, protocol: P) -> Result<Self, String>
    where
        P: BrowserProtocol,
    {
        self.protocols.register(protocol)?;
        Ok(self)
    }

    pub async fn spawn(self) -> Result<BrowserWebRtcNode, JsValue> {
        let registry = BrowserRtcRegistry::new();
        let runtime = BrowserRuntime::new(registry, self.protocols.clone());

        let result = runtime.spawn_facade_node(&self).await?;
        let endpoint_id = result.endpoint_id;

        Ok(BrowserWebRtcNode {
            inner: Rc::new(BrowserWebRtcNodeInner {
                endpoint_id,
                runtime,
            }),
        })
    }
}

impl BrowserWebRtcNode {
    pub fn builder(config: BrowserWebRtcNodeConfig) -> BrowserWebRtcNodeBuilder {
        BrowserWebRtcNodeBuilder {
            config,
            facade_alpns: Vec::new(),
            benchmark_echo_alpns: Vec::new(),
            protocols: BrowserProtocolRegistry::new(),
        }
    }

    pub async fn spawn(config: BrowserWebRtcNodeConfig) -> Result<Self, JsValue> {
        Self::builder(config).spawn().await
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
        let result = self
            .inner
            .runtime
            .dial_facade(remote_endpoint, alpn, options)
            .await?;
        Ok(BrowserWebRtcConnection::from_result(
            self.inner.clone(),
            result,
        ))
    }

    pub async fn accept(&self, alpn: &[u8]) -> Result<BrowserWebRtcAcceptor, JsValue> {
        let result = self.inner.runtime.accept_open_facade(alpn).await?;
        Ok(BrowserWebRtcAcceptor {
            node: self.inner.clone(),
            accept_key: result.accept_key,
            closed: Rc::new(std::cell::Cell::new(false)),
        })
    }

    pub async fn protocol<P>(&self) -> Result<BrowserProtocolHandle<P>, JsValue>
    where
        P: BrowserProtocol,
    {
        Ok(BrowserProtocolHandle {
            node: self.inner.clone(),
            _protocol: std::marker::PhantomData,
        })
    }

    pub async fn open_latency_echo(&self, alpn: &[u8]) -> Result<(), JsValue> {
        self.inner.runtime.open_latency_echo_facade(alpn).await
    }

    pub async fn check_latency(
        &self,
        remote_endpoint: &str,
        alpn: &[u8],
        samples: usize,
        warmups: usize,
    ) -> Result<BrowserLatencyStats, JsValue> {
        self.inner
            .runtime
            .check_latency_facade(remote_endpoint, alpn, samples, warmups)
            .await
    }

    pub async fn check_throughput(
        &self,
        remote_endpoint: &str,
        alpn: &[u8],
        bytes: usize,
        warmup_bytes: usize,
    ) -> Result<BrowserThroughputStats, JsValue> {
        self.inner
            .runtime
            .check_throughput_facade(remote_endpoint, alpn, bytes, warmup_bytes)
            .await
    }

    pub async fn close(&self, reason: &str) -> Result<(), JsValue> {
        self.inner.runtime.node_close_facade(reason).await?;
        self.inner.runtime.close();
        Ok(())
    }
}

#[derive(Clone)]
pub struct BrowserProtocolHandle<P> {
    node: Rc<BrowserWebRtcNodeInner>,
    _protocol: std::marker::PhantomData<P>,
}

impl<P> BrowserProtocolHandle<P>
where
    P: BrowserProtocol,
{
    pub async fn send(&self, command: P::Command) -> Result<(), JsValue> {
        self.node
            .runtime
            .protocol_command_facade::<P>(command)
            .await
    }

    pub async fn next_event(&self) -> Result<Option<P::Event>, JsValue> {
        self.node.runtime.protocol_next_event_facade::<P>().await
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
        let result = self
            .node
            .runtime
            .accept_next_facade(&self.accept_key)
            .await?;
        if result.done {
            self.closed.set(true);
            return Ok(None);
        }
        let connection = result
            .connection
            .ok_or_else(|| JsValue::from_str("browser accept result missing connection"))?;
        Ok(Some(BrowserWebRtcConnection::from_result(
            self.node.clone(),
            connection,
        )))
    }

    pub async fn close(&self, reason: &str) -> Result<(), JsValue> {
        if self.closed.replace(true) {
            return Ok(());
        }
        self.node
            .runtime
            .accept_close_facade(&self.accept_key, reason)
            .await
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
    fn from_result(node: Rc<BrowserWebRtcNodeInner>, result: BrowserConnectionInfo) -> Self {
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
        let result = self
            .node
            .runtime
            .stream_open_bi_facade(&self.connection_key)
            .await?;
        Ok(BrowserWebRtcStream::from_result(self.node.clone(), result))
    }

    pub async fn accept_bi(&self) -> Result<BrowserWebRtcStream, JsValue> {
        let result = self
            .node
            .runtime
            .stream_accept_bi_facade(&self.connection_key)
            .await?;
        if result.done {
            return Err(JsValue::from_str("connection stream accept completed"));
        }
        let stream_key = result
            .stream_key
            .ok_or_else(|| JsValue::from_str("browser stream accept result missing streamKey"))?;
        Ok(BrowserWebRtcStream {
            node: self.node.clone(),
            stream_key,
        })
    }

    pub async fn close(&self, reason: &str) -> Result<(), JsValue> {
        self.node
            .runtime
            .connection_close_facade(&self.connection_key, reason)
            .await
    }
}

#[derive(Clone)]
pub struct BrowserWebRtcStream {
    node: Rc<BrowserWebRtcNodeInner>,
    stream_key: String,
}

impl BrowserWebRtcStream {
    fn from_result(node: Rc<BrowserWebRtcNodeInner>, result: BrowserStreamInfo) -> Self {
        Self {
            node,
            stream_key: result.stream_key,
        }
    }

    pub async fn send_all(&self, bytes: &[u8]) -> Result<(), JsValue> {
        self.node
            .runtime
            .stream_send_chunk_facade(&self.stream_key, bytes)
            .await
    }

    pub async fn close_send(&self) -> Result<(), JsValue> {
        self.node
            .runtime
            .stream_close_send_facade(&self.stream_key)
            .await
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
        self.node
            .runtime
            .stream_receive_chunk_facade(&self.stream_key)
            .await
    }
}

impl BrowserRuntime {
    async fn spawn_facade_node(
        &self,
        builder: &BrowserWebRtcNodeBuilder,
    ) -> Result<BrowserNodeInfo, JsValue> {
        self.spawn(
            builder.config.stun_urls.clone(),
            builder.config.low_latency_quic_acks,
            builder.config.protocol_transport_preference.as_intent(),
            builder.facade_alpns.clone(),
            builder.benchmark_echo_alpns.clone(),
        )
        .await
    }

    async fn dial_facade(
        &self,
        remote_endpoint: &str,
        alpn: &[u8],
        options: BrowserDialOptions,
    ) -> Result<BrowserConnectionInfo, JsValue> {
        self.dial(
            parse_endpoint_addr(remote_endpoint)?,
            alpn_string(alpn),
            options.transport_preference.as_intent(),
        )
        .await
    }

    async fn accept_open_facade(
        &self,
        alpn: &[u8],
    ) -> Result<BrowserAcceptRegistrationInfo, JsValue> {
        self.accept_open(alpn_string(alpn))
    }

    async fn accept_next_facade(&self, accept_key: &str) -> Result<BrowserAcceptNextInfo, JsValue> {
        self.accept_next(accept_key).await
    }

    async fn accept_close_facade(&self, accept_key: &str, _reason: &str) -> Result<(), JsValue> {
        self.accept_close(accept_key)
    }

    async fn protocol_command_facade<P>(&self, command: P::Command) -> Result<(), JsValue>
    where
        P: BrowserProtocol,
    {
        self.send_protocol_command::<P>(command).await
    }

    async fn protocol_next_event_facade<P>(&self) -> Result<Option<P::Event>, JsValue>
    where
        P: BrowserProtocol,
    {
        self.next_protocol_event::<P>().await
    }

    async fn connection_close_facade(
        &self,
        connection_key: &str,
        reason: &str,
    ) -> Result<(), JsValue> {
        self.close_connection(connection_key, Some(reason.to_owned()))?;
        Ok(())
    }

    async fn node_close_facade(&self, reason: &str) -> Result<(), JsValue> {
        self.close_node(Some(reason.to_owned()))?;
        Ok(())
    }

    async fn stream_open_bi_facade(
        &self,
        connection_key: &str,
    ) -> Result<BrowserStreamInfo, JsValue> {
        self.open_bi(connection_key).await
    }

    async fn stream_accept_bi_facade(
        &self,
        connection_key: &str,
    ) -> Result<BrowserStreamAcceptInfo, JsValue> {
        self.accept_bi(connection_key).await
    }

    async fn stream_send_chunk_facade(
        &self,
        stream_key: &str,
        chunk: &[u8],
    ) -> Result<(), JsValue> {
        self.send_stream_chunk(stream_key, chunk, false).await?;
        Ok(())
    }

    async fn stream_close_send_facade(&self, stream_key: &str) -> Result<(), JsValue> {
        self.close_stream_send(stream_key).await?;
        Ok(())
    }

    async fn stream_receive_chunk_facade(
        &self,
        stream_key: &str,
    ) -> Result<Option<Vec<u8>>, JsValue> {
        let result = self.receive_stream_chunk(stream_key, None).await?;
        Ok((!result.done).then_some(result.chunk.unwrap_or_default()))
    }

    async fn open_latency_echo_facade(&self, alpn: &[u8]) -> Result<(), JsValue> {
        self.open_benchmark_echo(alpn_string(alpn))
    }

    async fn check_latency_facade(
        &self,
        remote_endpoint: &str,
        alpn: &[u8],
        samples: usize,
        warmups: usize,
    ) -> Result<BrowserLatencyStats, JsValue> {
        self.check_latency(
            parse_endpoint_addr(remote_endpoint)?,
            alpn_string(alpn),
            samples,
            warmups,
            BootstrapTransportIntent::WebRtcOnly,
        )
        .await
        .map(Into::into)
    }

    async fn check_throughput_facade(
        &self,
        remote_endpoint: &str,
        alpn: &[u8],
        bytes: usize,
        warmup_bytes: usize,
    ) -> Result<BrowserThroughputStats, JsValue> {
        self.check_throughput(
            parse_endpoint_addr(remote_endpoint)?,
            alpn_string(alpn),
            bytes,
            warmup_bytes,
            BootstrapTransportIntent::WebRtcOnly,
        )
        .await
        .map(Into::into)
    }
}

fn alpn_string(alpn: &[u8]) -> String {
    String::from_utf8_lossy(alpn).into_owned()
}

fn parse_endpoint_addr(endpoint_id: &str) -> Result<EndpointAddr, JsValue> {
    EndpointId::from_str(endpoint_id)
        .map(EndpointAddr::from)
        .map_err(|err| JsValue::from_str(&format!("invalid browser remote endpoint id: {err}")))
}

impl From<BrowserRuntimeLatencyStats> for BrowserLatencyStats {
    fn from(value: BrowserRuntimeLatencyStats) -> Self {
        Self {
            samples: value.samples,
            avg_ms: value.avg_ms,
            p50_ms: value.p50_ms,
            p95_ms: value.p95_ms,
            min_ms: value.min_ms,
            max_ms: value.max_ms,
        }
    }
}

impl From<BrowserRuntimeThroughputStats> for BrowserThroughputStats {
    fn from(value: BrowserRuntimeThroughputStats) -> Self {
        Self {
            bytes: value.bytes,
            upload_ms: value.upload_ms,
            download_ms: value.download_ms,
        }
    }
}
