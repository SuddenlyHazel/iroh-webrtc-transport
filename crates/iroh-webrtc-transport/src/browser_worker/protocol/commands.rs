use super::*;
use serde::de::DeserializeOwned;

pub(in crate::browser_worker) enum WorkerCommand {
    Spawn {
        config: BrowserWorkerNodeConfig,
    },
    Dial {
        remote_addr: EndpointAddr,
        alpn: String,
        transport_intent: BootstrapTransportIntent,
    },
    AcceptOpen {
        alpn: String,
    },
    AcceptNext {
        accept_id: WorkerAcceptId,
    },
    AcceptClose {
        accept_id: WorkerAcceptId,
    },
    BootstrapSignal {
        input: WorkerBootstrapSignalInput,
    },
    AttachDataChannel {
        session_key: WorkerSessionKey,
        source: WorkerDataChannelSource,
    },
    MainRtcResult {
        input: WorkerMainRtcResultInput,
    },
    ConnectionClose {
        connection_key: WorkerConnectionKey,
        reason: Option<String>,
    },
    NodeClose {
        reason: Option<String>,
    },
    StreamOpenBi {
        connection_key: WorkerConnectionKey,
    },
    StreamAcceptBi {
        connection_key: WorkerConnectionKey,
    },
    StreamSendChunk {
        stream_key: WorkerStreamKey,
        chunk: Vec<u8>,
        end_stream: bool,
    },
    StreamReceiveChunk {
        stream_key: WorkerStreamKey,
        max_bytes: Option<usize>,
    },
    StreamCloseSend {
        stream_key: WorkerStreamKey,
    },
    StreamReset {
        stream_key: WorkerStreamKey,
        reason: Option<String>,
    },
    StreamCancelPending {
        request_id: u64,
        stream_key: Option<WorkerStreamKey>,
        reason: Option<String>,
    },
    BenchmarkEchoOpen {
        alpn: String,
    },
    BenchmarkLatency {
        remote_addr: EndpointAddr,
        alpn: String,
        samples: usize,
        warmups: usize,
        transport_intent: BootstrapTransportIntent,
    },
    BenchmarkThroughput {
        remote_addr: EndpointAddr,
        alpn: String,
        bytes: usize,
        warmup_bytes: usize,
        transport_intent: BootstrapTransportIntent,
    },
}

impl WorkerCommand {
    pub(in crate::browser_worker) fn requires_open_node(&self) -> bool {
        !matches!(self, Self::Spawn { .. } | Self::NodeClose { .. })
    }

    pub(in crate::browser_worker) fn decode(
        command: &str,
        payload: Value,
    ) -> BrowserWorkerResult<Self> {
        match command {
            WORKER_SPAWN_COMMAND => {
                let payload: SpawnPayload = decode_payload(command, payload)?;
                Ok(Self::Spawn {
                    config: payload.into_config()?,
                })
            }
            WORKER_DIAL_COMMAND => {
                let payload: DialPayload = decode_payload(command, payload)?;
                let remote = parse_endpoint_id(
                    &payload.remote_endpoint,
                    "remoteEndpoint",
                    BrowserWorkerErrorCode::InvalidRemote,
                )?;
                Ok(Self::Dial {
                    remote_addr: endpoint_addr_from_remote_direct_addrs(
                        remote,
                        &payload.remote_direct_addrs,
                    )?,
                    alpn: validate_alpn(payload.alpn)?,
                    transport_intent: payload.transport_intent,
                })
            }
            WORKER_ACCEPT_OPEN_COMMAND => {
                let payload: AlpnPayload = decode_payload(command, payload)?;
                Ok(Self::AcceptOpen {
                    alpn: validate_alpn(payload.alpn)?,
                })
            }
            WORKER_ACCEPT_NEXT_COMMAND => Ok(Self::AcceptNext {
                accept_id: decode_payload::<AcceptKeyPayload>(command, payload)?.accept_id()?,
            }),
            WORKER_ACCEPT_CLOSE_COMMAND => Ok(Self::AcceptClose {
                accept_id: decode_payload::<AcceptKeyPayload>(command, payload)?.accept_id()?,
            }),
            WORKER_BOOTSTRAP_SIGNAL_COMMAND => Ok(Self::BootstrapSignal {
                input: bootstrap_signal_input_from_payload(&payload)?,
            }),
            WORKER_ATTACH_DATA_CHANNEL_COMMAND => {
                let payload: AttachDataChannelPayload = decode_payload(command, payload)?;
                Ok(Self::AttachDataChannel {
                    session_key: WorkerSessionKey::new(payload.session_key)?,
                    source: payload.source,
                })
            }
            WORKER_MAIN_RTC_RESULT_COMMAND => Ok(Self::MainRtcResult {
                input: main_rtc_result_input_from_payload(&payload)?,
            }),
            WORKER_CONNECTION_CLOSE_COMMAND => {
                let payload: ConnectionReasonPayload = decode_payload(command, payload)?;
                Ok(Self::ConnectionClose {
                    connection_key: payload.connection_key()?,
                    reason: payload.reason,
                })
            }
            WORKER_NODE_CLOSE_COMMAND => Ok(Self::NodeClose {
                reason: decode_payload::<ReasonPayload>(command, payload)?.reason,
            }),
            STREAM_OPEN_BI_COMMAND => Ok(Self::StreamOpenBi {
                connection_key: decode_payload::<ConnectionKeyPayload>(command, payload)?
                    .connection_key()?,
            }),
            STREAM_ACCEPT_BI_COMMAND => Ok(Self::StreamAcceptBi {
                connection_key: decode_payload::<ConnectionKeyPayload>(command, payload)?
                    .connection_key()?,
            }),
            STREAM_SEND_CHUNK_COMMAND => {
                let payload: StreamSendChunkPayload = decode_payload(command, payload)?;
                Ok(Self::StreamSendChunk {
                    stream_key: payload.stream_key()?,
                    chunk: payload.chunk,
                    end_stream: payload.end_stream,
                })
            }
            STREAM_RECEIVE_CHUNK_COMMAND => {
                let payload: StreamReceiveChunkPayload = decode_payload(command, payload)?;
                Ok(Self::StreamReceiveChunk {
                    stream_key: payload.stream_key()?,
                    max_bytes: payload.max_bytes,
                })
            }
            STREAM_CLOSE_SEND_COMMAND => Ok(Self::StreamCloseSend {
                stream_key: decode_payload::<StreamKeyPayload>(command, payload)?.stream_key()?,
            }),
            STREAM_RESET_COMMAND => {
                let payload: StreamReasonPayload = decode_payload(command, payload)?;
                Ok(Self::StreamReset {
                    stream_key: payload.stream_key()?,
                    reason: payload.reason,
                })
            }
            STREAM_CANCEL_PENDING_COMMAND => {
                let payload: StreamCancelPendingPayload = decode_payload(command, payload)?;
                Ok(Self::StreamCancelPending {
                    request_id: payload.request_id,
                    stream_key: payload.optional_stream_key()?,
                    reason: payload.reason,
                })
            }
            BENCHMARK_ECHO_OPEN_COMMAND => {
                let payload: AlpnPayload = decode_payload(command, payload)?;
                Ok(Self::BenchmarkEchoOpen {
                    alpn: validate_alpn(payload.alpn)?,
                })
            }
            BENCHMARK_LATENCY_COMMAND => {
                let payload: BenchmarkLatencyPayload = decode_payload(command, payload)?;
                let remote = parse_endpoint_id(
                    &payload.remote_endpoint,
                    "remoteEndpoint",
                    BrowserWorkerErrorCode::InvalidRemote,
                )?;
                Ok(Self::BenchmarkLatency {
                    remote_addr: endpoint_addr_from_remote_direct_addrs(
                        remote,
                        &payload.remote_direct_addrs,
                    )?,
                    alpn: validate_alpn(payload.alpn)?,
                    samples: payload.samples,
                    warmups: payload.warmups,
                    transport_intent: payload.transport_intent,
                })
            }
            BENCHMARK_THROUGHPUT_COMMAND => {
                let payload: BenchmarkThroughputPayload = decode_payload(command, payload)?;
                let remote = parse_endpoint_id(
                    &payload.remote_endpoint,
                    "remoteEndpoint",
                    BrowserWorkerErrorCode::InvalidRemote,
                )?;
                Ok(Self::BenchmarkThroughput {
                    remote_addr: endpoint_addr_from_remote_direct_addrs(
                        remote,
                        &payload.remote_direct_addrs,
                    )?,
                    alpn: validate_alpn(payload.alpn)?,
                    bytes: payload.bytes,
                    warmup_bytes: payload.warmup_bytes,
                    transport_intent: payload.transport_intent,
                })
            }
            _ => Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("unsupported worker command {command:?}"),
            )),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpawnPayload {
    #[serde(default)]
    stun_urls: Vec<String>,
    #[serde(default)]
    accept_queue_capacity: Option<usize>,
    #[serde(default)]
    low_latency_quic_acks: bool,
}

impl SpawnPayload {
    fn into_config(self) -> BrowserWorkerResult<BrowserWorkerNodeConfig> {
        let mut config = BrowserWorkerNodeConfig::default();
        if !self.stun_urls.is_empty() {
            config.session_config =
                WebRtcSessionConfig::direct_stun(self.stun_urls).map_err(|err| {
                    BrowserWorkerError::new(BrowserWorkerErrorCode::InvalidStunUrl, err.to_string())
                })?;
        }
        if let Some(capacity) = self.accept_queue_capacity {
            config.accept_queue_capacity = capacity;
        }
        config.low_latency_quic_acks = self.low_latency_quic_acks;
        config.validate()?;
        Ok(config)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DialPayload {
    remote_endpoint: String,
    #[serde(default)]
    remote_direct_addrs: Vec<String>,
    alpn: String,
    transport_intent: BootstrapTransportIntent,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlpnPayload {
    alpn: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptKeyPayload {
    accept_key: String,
}

impl AcceptKeyPayload {
    fn accept_id(self) -> BrowserWorkerResult<WorkerAcceptId> {
        parse_prefixed_string_key(
            &self.accept_key,
            "accept-",
            "acceptKey",
            BrowserWorkerErrorCode::UnsupportedAlpn,
        )
        .map(WorkerAcceptId)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttachDataChannelPayload {
    session_key: String,
    #[serde(rename = "generation")]
    #[serde(default)]
    _generation: Option<u64>,
    source: WorkerDataChannelSource,
    #[serde(rename = "channel")]
    #[serde(default)]
    _channel: Option<Value>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionKeyPayload {
    connection_key: String,
}

impl ConnectionKeyPayload {
    fn connection_key(self) -> BrowserWorkerResult<WorkerConnectionKey> {
        parse_connection_key(&self.connection_key)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionReasonPayload {
    connection_key: String,
    #[serde(default)]
    reason: Option<String>,
}

impl ConnectionReasonPayload {
    fn connection_key(&self) -> BrowserWorkerResult<WorkerConnectionKey> {
        parse_connection_key(&self.connection_key)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReasonPayload {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamKeyPayload {
    stream_key: String,
}

impl StreamKeyPayload {
    fn stream_key(self) -> BrowserWorkerResult<WorkerStreamKey> {
        parse_stream_key(&self.stream_key)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamSendChunkPayload {
    stream_key: String,
    #[serde(default)]
    chunk: Vec<u8>,
    #[serde(default)]
    end_stream: bool,
}

impl StreamSendChunkPayload {
    fn stream_key(&self) -> BrowserWorkerResult<WorkerStreamKey> {
        parse_stream_key(&self.stream_key)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamReceiveChunkPayload {
    stream_key: String,
    #[serde(default)]
    max_bytes: Option<usize>,
}

impl StreamReceiveChunkPayload {
    fn stream_key(&self) -> BrowserWorkerResult<WorkerStreamKey> {
        parse_stream_key(&self.stream_key)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamReasonPayload {
    stream_key: String,
    #[serde(default)]
    reason: Option<String>,
}

impl StreamReasonPayload {
    fn stream_key(&self) -> BrowserWorkerResult<WorkerStreamKey> {
        parse_stream_key(&self.stream_key)
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StreamCancelPendingPayload {
    request_id: u64,
    #[serde(default)]
    stream_key: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

impl StreamCancelPendingPayload {
    fn optional_stream_key(&self) -> BrowserWorkerResult<Option<WorkerStreamKey>> {
        self.stream_key.as_deref().map(parse_stream_key).transpose()
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BenchmarkLatencyPayload {
    remote_endpoint: String,
    #[serde(default)]
    remote_direct_addrs: Vec<String>,
    alpn: String,
    #[serde(default = "default_latency_samples")]
    samples: usize,
    #[serde(default = "default_latency_warmups")]
    warmups: usize,
    transport_intent: BootstrapTransportIntent,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BenchmarkThroughputPayload {
    remote_endpoint: String,
    #[serde(default)]
    remote_direct_addrs: Vec<String>,
    alpn: String,
    #[serde(default = "default_throughput_bytes")]
    bytes: usize,
    #[serde(default = "default_throughput_warmup_bytes")]
    warmup_bytes: usize,
    transport_intent: BootstrapTransportIntent,
}

fn decode_payload<T: DeserializeOwned>(command: &str, payload: Value) -> BrowserWorkerResult<T> {
    serde_json::from_value(payload).map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("malformed payload for {command}: {err}"),
        )
    })
}

fn parse_endpoint_id(
    value: &str,
    field: &str,
    code: BrowserWorkerErrorCode,
) -> BrowserWorkerResult<EndpointId> {
    EndpointId::from_str(value).map_err(|err| {
        BrowserWorkerError::new(
            code,
            format!("payload field {field:?} is not a valid endpoint id: {err}"),
        )
    })
}

fn endpoint_addr_from_remote_direct_addrs(
    remote: EndpointId,
    remote_direct_addrs: &[String],
) -> BrowserWorkerResult<EndpointAddr> {
    if remote_direct_addrs.is_empty() {
        return Ok(EndpointAddr::from(remote));
    }
    let addrs = remote_direct_addrs
        .iter()
        .map(|addr| {
            SocketAddr::from_str(addr)
                .map(TransportAddr::Ip)
                .map_err(|err| {
                    BrowserWorkerError::new(
                        BrowserWorkerErrorCode::InvalidRemote,
                        format!("remoteDirectAddrs contains an invalid socket address: {err}"),
                    )
                })
        })
        .collect::<BrowserWorkerResult<Vec<_>>>()?;
    Ok(EndpointAddr::from_parts(remote, addrs))
}

fn parse_connection_key(value: &str) -> BrowserWorkerResult<WorkerConnectionKey> {
    parse_prefixed_string_key(
        value,
        "conn-",
        "connectionKey",
        BrowserWorkerErrorCode::WebRtcFailed,
    )
    .map(WorkerConnectionKey)
}

fn parse_stream_key(value: &str) -> BrowserWorkerResult<WorkerStreamKey> {
    parse_prefixed_string_key(
        value,
        "stream-",
        "streamKey",
        BrowserWorkerErrorCode::WebRtcFailed,
    )
    .map(WorkerStreamKey)
}

fn parse_prefixed_string_key(
    value: &str,
    prefix: &str,
    field: &str,
    code: BrowserWorkerErrorCode,
) -> BrowserWorkerResult<u64> {
    let raw = value.strip_prefix(prefix).ok_or_else(|| {
        BrowserWorkerError::new(
            code,
            format!("payload field {field:?} must start with {prefix:?}"),
        )
    })?;
    raw.parse::<u64>().map_err(|err| {
        BrowserWorkerError::new(
            code,
            format!("payload field {field:?} has an invalid numeric suffix: {err}"),
        )
    })
}

const fn default_latency_samples() -> usize {
    25
}

const fn default_latency_warmups() -> usize {
    3
}

const fn default_throughput_bytes() -> usize {
    1024 * 1024
}

const fn default_throughput_warmup_bytes() -> usize {
    256 * 1024
}
