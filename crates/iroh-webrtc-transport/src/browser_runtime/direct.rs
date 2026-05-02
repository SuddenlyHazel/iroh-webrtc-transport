use std::{cell::RefCell, rc::Rc};

use wasm_bindgen::JsValue;

use super::*;

mod benchmark;
mod bootstrap;
mod data_channel;
mod dial;
mod js_boundary;
mod protocol_transport;
mod rtc_registry;

use bootstrap::{BrowserBootstrapRuntime, send_outbound_signals, start_bootstrap_accept_loop};
use dial::dial_application_connection;
use js_boundary::runtime_error_to_js;
use protocol_transport::start_protocol_transport_prepare_loop;
use rtc_registry::BrowserRtcControl;

#[derive(Clone)]
pub(crate) struct BrowserRuntime {
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BrowserRuntimeLatencyStats {
    pub(crate) samples: usize,
    pub(crate) avg_ms: f64,
    pub(crate) p50_ms: f64,
    pub(crate) p95_ms: f64,
    pub(crate) min_ms: f64,
    pub(crate) max_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BrowserRuntimeThroughputStats {
    pub(crate) bytes: usize,
    pub(crate) upload_ms: f64,
    pub(crate) download_ms: f64,
}

impl BrowserRuntime {
    pub(crate) fn new(
        registry: crate::browser::BrowserRtcRegistry,
        protocols: BrowserProtocolRegistry,
    ) -> Self {
        crate::browser_console_tracing::install_browser_console_tracing();
        let (protocol_transport_prepare_tx, protocol_transport_prepare_rx) = mpsc::channel(32);
        let core = Rc::new(
            BrowserRuntimeCore::new_with_protocols_and_transport_prepare(
                protocols,
                protocol_transport_prepare_tx,
            ),
        );
        let rtc_control = Rc::new(RefCell::new(Some(BrowserRtcControl::new(registry))));
        let bootstrap = Rc::new(BrowserBootstrapRuntime::new());
        start_protocol_transport_prepare_loop(
            core.clone(),
            rtc_control.clone(),
            bootstrap.clone(),
            protocol_transport_prepare_rx,
        );
        Self {
            core,
            rtc_control,
            bootstrap,
        }
    }

    pub(crate) async fn spawn(
        &self,
        stun_urls: Vec<String>,
        secret_key: SecretKey,
        low_latency_quic_acks: bool,
        protocol_transport_intent: BootstrapTransportIntent,
        facade_alpns: Vec<String>,
        benchmark_echo_alpns: Vec<String>,
    ) -> Result<BrowserNodeInfo, JsValue> {
        start_bootstrap_accept_loop(
            self.core.clone(),
            self.rtc_control.clone(),
            self.bootstrap.clone(),
        );
        let config = browser_runtime_config(
            stun_urls,
            low_latency_quic_acks,
            protocol_transport_intent,
            facade_alpns,
            benchmark_echo_alpns,
        )
        .map_err(|err| runtime_error_to_js(err))?;
        let result = self
            .core
            .spawn_node(
                config,
                secret_key,
                Some(self.bootstrap.connection_tx.clone()),
            )
            .await
            .map_err(|err| runtime_error_to_js(err))?;
        Ok(result)
    }

    pub(crate) async fn dial(
        &self,
        remote_addr: EndpointAddr,
        alpn: String,
        transport_intent: BootstrapTransportIntent,
    ) -> Result<BrowserConnectionInfo, JsValue> {
        dial_application_connection(
            &self.core,
            &self.rtc_control,
            &self.bootstrap,
            remote_addr,
            alpn,
            transport_intent,
        )
        .await
        .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) fn accept_open(
        &self,
        alpn: String,
    ) -> Result<BrowserAcceptRegistrationInfo, JsValue> {
        self.core
            .open_node()
            .and_then(|node| node.accept_open_result(alpn))
            .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) async fn accept_next(
        &self,
        accept_key: &str,
    ) -> Result<BrowserAcceptNextInfo, JsValue> {
        let accept_id = parse_accept_key(accept_key)?;
        self.core
            .open_node()
            .map_err(|err| runtime_error_to_js(err))?
            .accept_next_result(accept_id)
            .await
            .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) fn accept_close(&self, accept_key: &str) -> Result<(), JsValue> {
        let accept_id = parse_accept_key(accept_key)?;
        if !self
            .core
            .open_node()
            .and_then(|node| node.accept_close(accept_id))
            .map_err(|err| runtime_error_to_js(err))?
        {
            return Err(JsValue::from_str("unknown accept registration"));
        }
        Ok(())
    }

    pub(crate) async fn send_protocol_command<P>(&self, command: P::Command) -> Result<(), JsValue>
    where
        P: crate::browser::BrowserProtocol,
    {
        self.core
            .send_protocol_command::<P>(command)
            .await
            .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) async fn next_protocol_event<P>(&self) -> Result<Option<P::Event>, JsValue>
    where
        P: crate::browser::BrowserProtocol,
    {
        self.core
            .next_protocol_event::<P>()
            .await
            .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) fn close_connection(
        &self,
        connection_key: &str,
        reason: Option<String>,
    ) -> Result<BrowserCloseOutcome, JsValue> {
        let connection_key = parse_connection_key(connection_key)?;
        let result = self
            .core
            .open_node()
            .and_then(|node| node.connection_close(connection_key, reason))
            .map_err(|err| runtime_error_to_js(err))?;
        send_outbound_signals(&self.bootstrap, &result.outbound_signals)?;
        Ok(result)
    }

    pub(crate) fn close_node(
        &self,
        reason: Option<String>,
    ) -> Result<BrowserCloseOutcome, JsValue> {
        let result = self
            .core
            .node()
            .map(|node| node.node_close_result(reason))
            .unwrap_or_else(|| BrowserCloseOutcome {
                closed: true,
                outbound_signals: Vec::new(),
            });
        send_outbound_signals(&self.bootstrap, &result.outbound_signals)?;
        self.close();
        Ok(result)
    }

    pub(crate) async fn open_bi(&self, connection_key: &str) -> Result<BrowserStreamInfo, JsValue> {
        self.core
            .open_node()
            .map_err(|err| runtime_error_to_js(err))?
            .open_bi(parse_connection_key(connection_key)?)
            .await
            .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) async fn accept_bi(
        &self,
        connection_key: &str,
    ) -> Result<BrowserStreamAcceptInfo, JsValue> {
        self.core
            .open_node()
            .map_err(|err| runtime_error_to_js(err))?
            .accept_bi(parse_connection_key(connection_key)?)
            .await
            .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) async fn send_stream_chunk(
        &self,
        stream_key: &str,
        chunk: &[u8],
        end_stream: bool,
    ) -> Result<BrowserStreamSendOutcome, JsValue> {
        self.core
            .open_node()
            .map_err(|err| runtime_error_to_js(err))?
            .send_stream_chunk(parse_stream_key(stream_key)?, chunk, end_stream)
            .await
            .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) async fn receive_stream_chunk(
        &self,
        stream_key: &str,
        max_bytes: Option<usize>,
    ) -> Result<BrowserStreamReceiveOutcome, JsValue> {
        self.core
            .open_node()
            .map_err(|err| runtime_error_to_js(err))?
            .receive_stream_chunk(parse_stream_key(stream_key)?, max_bytes)
            .await
            .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) async fn close_stream_send(
        &self,
        stream_key: &str,
    ) -> Result<BrowserStreamCloseSendOutcome, JsValue> {
        self.core
            .open_node()
            .map_err(|err| runtime_error_to_js(err))?
            .close_stream_send(parse_stream_key(stream_key)?)
            .await
            .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) fn open_benchmark_echo(&self, alpn: String) -> Result<(), JsValue> {
        let _ = self
            .core
            .open_node()
            .and_then(|node| node.open_benchmark_echo(alpn))
            .map_err(|err| runtime_error_to_js(err))?;
        Ok(())
    }

    pub(crate) async fn check_latency(
        &self,
        remote_addr: EndpointAddr,
        alpn: String,
        samples: usize,
        warmups: usize,
        transport_intent: BootstrapTransportIntent,
    ) -> Result<BrowserRuntimeLatencyStats, JsValue> {
        benchmark::check_latency(
            &self.core,
            &self.rtc_control,
            &self.bootstrap,
            remote_addr,
            alpn,
            samples,
            warmups,
            transport_intent,
        )
        .await
        .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) async fn check_throughput(
        &self,
        remote_addr: EndpointAddr,
        alpn: String,
        bytes: usize,
        warmup_bytes: usize,
        transport_intent: BootstrapTransportIntent,
    ) -> Result<BrowserRuntimeThroughputStats, JsValue> {
        benchmark::check_throughput(
            &self.core,
            &self.rtc_control,
            &self.bootstrap,
            remote_addr,
            alpn,
            bytes,
            warmup_bytes,
            transport_intent,
        )
        .await
        .map_err(|err| runtime_error_to_js(err))
    }

    pub(crate) fn close(&self) {
        *self.rtc_control.borrow_mut() = None;
    }
}

fn browser_runtime_config(
    stun_urls: Vec<String>,
    low_latency_quic_acks: bool,
    protocol_transport_intent: BootstrapTransportIntent,
    facade_alpns: Vec<String>,
    benchmark_echo_alpns: Vec<String>,
) -> BrowserRuntimeResult<BrowserRuntimeNodeConfig> {
    let mut config = BrowserRuntimeNodeConfig::default();
    if !stun_urls.is_empty() {
        config.session_config = WebRtcSessionConfig::direct_stun(stun_urls).map_err(|err| {
            BrowserRuntimeError::new(BrowserRuntimeErrorCode::InvalidStunUrl, err.to_string())
        })?;
    }
    config.low_latency_quic_acks = low_latency_quic_acks;
    config.protocol_transport_intent = protocol_transport_intent;
    config.facade_alpns = facade_alpns
        .into_iter()
        .map(validate_alpn)
        .collect::<BrowserRuntimeResult<Vec<_>>>()?;
    config.benchmark_echo_alpns = benchmark_echo_alpns
        .into_iter()
        .map(validate_alpn)
        .collect::<BrowserRuntimeResult<Vec<_>>>()?;
    config.validate()?;
    Ok(config)
}

fn parse_accept_key(value: &str) -> Result<BrowserAcceptId, JsValue> {
    parse_prefixed_key(
        value,
        "accept-",
        "acceptKey",
        BrowserRuntimeErrorCode::UnsupportedAlpn,
    )
    .map(BrowserAcceptId)
    .map_err(|err| runtime_error_to_js(err))
}

fn parse_connection_key(value: &str) -> Result<BrowserConnectionKey, JsValue> {
    parse_prefixed_key(
        value,
        "conn-",
        "connectionKey",
        BrowserRuntimeErrorCode::WebRtcFailed,
    )
    .map(BrowserConnectionKey)
    .map_err(|err| runtime_error_to_js(err))
}

fn parse_stream_key(value: &str) -> Result<BrowserStreamKey, JsValue> {
    parse_prefixed_key(
        value,
        "stream-",
        "streamKey",
        BrowserRuntimeErrorCode::WebRtcFailed,
    )
    .map(BrowserStreamKey)
    .map_err(|err| runtime_error_to_js(err))
}
