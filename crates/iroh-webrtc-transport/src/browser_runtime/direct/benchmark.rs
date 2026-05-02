use std::{cell::RefCell, rc::Rc};

use super::bootstrap::BrowserBootstrapRuntime;
use super::dial::dial_application_connection;
use super::rtc_registry::BrowserRtcControl;
use super::*;

pub(super) async fn check_latency(
    core: &Rc<BrowserRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    remote_addr: EndpointAddr,
    alpn: String,
    samples: usize,
    warmups: usize,
    transport_intent: BootstrapTransportIntent,
) -> BrowserRuntimeResult<BrowserRuntimeLatencyStats> {
    let remote = remote_addr.id;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::benchmark",
        browser_now_ms = benchmark_now_ms(),
        remote = %remote,
        alpn = %alpn,
        samples,
        warmups,
        "benchmark latency dial start"
    );
    let connection = dial_application_connection(
        core,
        rtc_control,
        bootstrap,
        remote_addr,
        alpn.clone(),
        transport_intent,
    )
    .await?;
    let connection_key = connection_key_from_string(&connection.connection_key)?;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::benchmark",
        browser_now_ms = benchmark_now_ms(),
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
        target: "iroh_webrtc_transport::browser_runtime::benchmark",
        browser_now_ms = benchmark_now_ms(),
        remote = %remote,
        alpn = %alpn,
        connection_key = connection_key.0,
        samples = result.samples,
        avg_ms = result.avg_ms,
        p50_ms = result.p50_ms,
        p95_ms = result.p95_ms,
        min_ms = result.min_ms,
        max_ms = result.max_ms,
        "benchmark latency check complete"
    );
    Ok(BrowserRuntimeLatencyStats {
        samples: result.samples,
        avg_ms: result.avg_ms,
        p50_ms: result.p50_ms,
        p95_ms: result.p95_ms,
        min_ms: result.min_ms,
        max_ms: result.max_ms,
    })
}

pub(super) async fn check_throughput(
    core: &Rc<BrowserRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    remote_addr: EndpointAddr,
    alpn: String,
    bytes: usize,
    warmup_bytes: usize,
    transport_intent: BootstrapTransportIntent,
) -> BrowserRuntimeResult<BrowserRuntimeThroughputStats> {
    let remote = remote_addr.id;
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::benchmark",
        browser_now_ms = benchmark_now_ms(),
        remote = %remote,
        alpn = %alpn,
        bytes,
        warmup_bytes,
        "benchmark throughput dial start"
    );
    let connection = dial_application_connection(
        core,
        rtc_control,
        bootstrap,
        remote_addr,
        alpn.clone(),
        transport_intent,
    )
    .await?;
    let connection_key = connection_key_from_string(&connection.connection_key)?;
    tracing::debug!(
        target: "iroh_webrtc_transport::browser_runtime::benchmark",
        browser_now_ms = benchmark_now_ms(),
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
        target: "iroh_webrtc_transport::browser_runtime::benchmark",
        browser_now_ms = benchmark_now_ms(),
        remote = %remote,
        alpn = %alpn,
        connection_key = connection_key.0,
        bytes = result.bytes,
        upload_ms = result.upload_ms,
        download_ms = result.download_ms,
        "benchmark throughput check complete"
    );
    Ok(BrowserRuntimeThroughputStats {
        bytes: result.bytes,
        upload_ms: result.upload_ms,
        download_ms: result.download_ms,
    })
}

fn benchmark_now_ms() -> f64 {
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

fn connection_key_from_string(value: &str) -> BrowserRuntimeResult<BrowserConnectionKey> {
    parse_prefixed_key(
        value,
        "conn-",
        "connectionKey",
        BrowserRuntimeErrorCode::WebRtcFailed,
    )
    .map(BrowserConnectionKey)
}
