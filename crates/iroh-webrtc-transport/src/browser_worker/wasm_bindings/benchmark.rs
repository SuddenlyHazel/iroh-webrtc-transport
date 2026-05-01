use std::{cell::RefCell, rc::Rc};

use super::bootstrap::WorkerBootstrapRuntime;
use super::dial::handle_dial_command_value;
use super::rtc_control::WorkerRtcControlPort;
use super::*;

pub(super) async fn handle_benchmark_latency_command_value(
    core: &Rc<BrowserWorkerRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: &Rc<WorkerBootstrapRuntime>,
    post_message: Option<&js_sys::Function>,
    payload: Value,
) -> BrowserWorkerResult<Value> {
    let WorkerCommand::BenchmarkLatency {
        remote_addr,
        alpn,
        samples,
        warmups,
        transport_intent,
    } = WorkerCommand::decode(BENCHMARK_LATENCY_COMMAND, payload.clone())?
    else {
        unreachable!("worker latency benchmark command decoded to another variant");
    };
    let remote = remote_addr.id;
    let dial_payload = benchmark_dial_payload(&remote_addr, &alpn, transport_intent);
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::benchmark",
        worker_now_ms = benchmark_command_now_ms(),
        remote = %remote,
        alpn = %alpn,
        samples,
        warmups,
        "benchmark latency dial start"
    );
    let connection =
        handle_dial_command_value(core, rtc_control, bootstrap, post_message, dial_payload).await?;
    let connection_key = required_connection_key(&connection, "connectionKey")?;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::benchmark",
        worker_now_ms = benchmark_command_now_ms(),
        remote = %remote,
        alpn = %alpn,
        connection_key = connection_key.0,
        "benchmark latency dial complete"
    );
    let result = core
        .open_node()?
        .benchmark_latency_on_connection(connection_key, samples, warmups)
        .await?;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::benchmark",
        worker_now_ms = benchmark_command_now_ms(),
        remote = %remote,
        alpn = %alpn,
        connection_key = connection_key.0,
        samples = result.samples,
        avg_ms = result.avg_ms,
        p50_ms = result.p50_ms,
        p95_ms = result.p95_ms,
        min_ms = result.min_ms,
        max_ms = result.max_ms,
        "benchmark latency command complete"
    );
    to_protocol_value(result)
}

pub(super) async fn handle_benchmark_throughput_command_value(
    core: &Rc<BrowserWorkerRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<WorkerRtcControlPort>>>,
    bootstrap: &Rc<WorkerBootstrapRuntime>,
    post_message: Option<&js_sys::Function>,
    payload: Value,
) -> BrowserWorkerResult<Value> {
    let WorkerCommand::BenchmarkThroughput {
        remote_addr,
        alpn,
        bytes,
        warmup_bytes,
        transport_intent,
    } = WorkerCommand::decode(BENCHMARK_THROUGHPUT_COMMAND, payload.clone())?
    else {
        unreachable!("worker throughput benchmark command decoded to another variant");
    };
    let remote = remote_addr.id;
    let dial_payload = benchmark_dial_payload(&remote_addr, &alpn, transport_intent);
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::benchmark",
        worker_now_ms = benchmark_command_now_ms(),
        remote = %remote,
        alpn = %alpn,
        bytes,
        warmup_bytes,
        "benchmark throughput dial start"
    );
    let connection =
        handle_dial_command_value(core, rtc_control, bootstrap, post_message, dial_payload).await?;
    let connection_key = required_connection_key(&connection, "connectionKey")?;
    tracing::debug!(
        target: "iroh_webrtc_transport::browser_worker::benchmark",
        worker_now_ms = benchmark_command_now_ms(),
        remote = %remote,
        alpn = %alpn,
        connection_key = connection_key.0,
        "benchmark throughput dial complete"
    );
    let result = core
        .open_node()?
        .benchmark_throughput_on_connection(connection_key, bytes, warmup_bytes)
        .await?;
    tracing::debug!(
        target: "iroh_webrtc_transport::browser_worker::benchmark",
        worker_now_ms = benchmark_command_now_ms(),
        remote = %remote,
        alpn = %alpn,
        connection_key = connection_key.0,
        bytes = result.bytes,
        upload_ms = result.upload_ms,
        download_ms = result.download_ms,
        "benchmark throughput command complete"
    );
    to_protocol_value(result)
}

fn benchmark_command_now_ms() -> f64 {
    performance_now_ms().unwrap_or_else(js_sys::Date::now)
}

fn performance_now_ms() -> Option<f64> {
    use wasm_bindgen::JsCast as _;

    let performance = js_sys::Reflect::get(
        &js_sys::global(),
        &wasm_bindgen::JsValue::from_str("performance"),
    )
    .ok()?;
    let now = js_sys::Reflect::get(&performance, &wasm_bindgen::JsValue::from_str("now")).ok()?;
    let now = now.dyn_into::<js_sys::Function>().ok()?;
    now.call0(&performance).ok()?.as_f64()
}

fn benchmark_dial_payload(
    remote_addr: &EndpointAddr,
    alpn: &str,
    transport_intent: BootstrapTransportIntent,
) -> Value {
    let remote_direct_addrs = remote_addr
        .addrs
        .iter()
        .filter_map(|addr| match addr {
            TransportAddr::Ip(addr) => Some(addr.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    json!({
        "remoteEndpoint": remote_addr.id.to_string(),
        "remoteDirectAddrs": remote_direct_addrs,
        "alpn": alpn,
        "transportIntent": transport_intent,
    })
}
