use iroh::SecretKey;
use iroh_webrtc_transport::browser::{
    BrowserDialOptions, BrowserWebRtcNode, BrowserWebRtcNodeConfig,
};
use qrcode::{QrCode, render::svg};
use wasm_bindgen::{JsCast, prelude::*};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Document, Element, HtmlButtonElement, HtmlInputElement, HtmlPreElement};

const ALPN: &[u8] = b"example/iroh-webrtc-rust-browser-ping-pong/1";
const BENCHMARK_ALPN: &[u8] = b"example/iroh-webrtc-rust-browser-ping-pong/benchmark/1";
const LATENCY_WARMUP_SAMPLES: usize = 16;
const THROUGHPUT_WARMUP_BYTES: usize = 256 * 1024;
const MAX_BENCHMARK_FRAME_BYTES: usize = 128 * 1024 * 1024;

iroh_webrtc_transport::browser_app! {
    app = start_app => run_app;
}

async fn run_app() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    iroh_webrtc_transport::browser::install_browser_console_tracing();

    let document = document()?;
    let local = element::<HtmlInputElement>(&document, "local")?;
    let local_qr = element::<Element>(&document, "local-qr")?;
    let remote = element::<HtmlInputElement>(&document, "remote")?;
    let ping = element::<HtmlButtonElement>(&document, "ping")?;
    let latency = element::<HtmlButtonElement>(&document, "latency")?;
    let latency_samples = element::<HtmlInputElement>(&document, "latency-samples")?;
    let throughput = element::<HtmlButtonElement>(&document, "throughput")?;
    let throughput_mib = element::<HtmlInputElement>(&document, "throughput-mib")?;
    let log = element::<HtmlPreElement>(&document, "log")?;
    let action_buttons = vec![ping.clone(), latency.clone(), throughput.clone()];

    let low_latency_quic_acks = query_flag("lowLatencyQuicAcks")?;
    let secret_key = SecretKey::generate();
    let node = BrowserWebRtcNode::builder(
        BrowserWebRtcNodeConfig::default().with_low_latency_quic_acks(low_latency_quic_acks),
        secret_key,
    )
    .accept_facade(ALPN)
    .accept_latency_echo(BENCHMARK_ALPN)
    .spawn()
    .await?;
    let endpoint_id = node.endpoint_id().to_string();
    local.set_value(&endpoint_id);
    render_endpoint_qr(&local_qr, &endpoint_id)?;
    node.open_latency_echo(BENCHMARK_ALPN).await?;
    log_line(&log, &format!("local endpoint: {endpoint_id}"));
    log_line(
        &log,
        if low_latency_quic_acks {
            "quic ack mode: low-latency"
        } else {
            "quic ack mode: iroh-default"
        },
    );

    let accept_node = node.clone();
    let accept_log = log.clone();
    spawn_local(async move {
        if let Err(error) = accept_connections(accept_node, accept_log.clone()).await {
            log_line(&accept_log, &format!("accept error: {error:?}"));
        }
    });

    let click_log = log.clone();
    let click_remote = remote.clone();
    let click_node = node.clone();
    let click_buttons = action_buttons.clone();
    let on_click = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        let log = click_log.clone();
        let remote = click_remote.value();
        let node = click_node.clone();
        let buttons = click_buttons.clone();
        set_buttons_disabled(&buttons, true);
        spawn_local(async move {
            if let Err(error) = ping_remote(node, remote, log.clone()).await {
                log_line(&log, &format!("error: {error:?}"));
            }
            set_buttons_disabled(&buttons, false);
        });
    }) as Box<dyn FnMut(_)>);

    ping.add_event_listener_with_callback("click", on_click.as_ref().unchecked_ref())?;
    on_click.forget();

    let latency_log = log.clone();
    let latency_remote = remote.clone();
    let latency_samples_input = latency_samples.clone();
    let latency_node = node.clone();
    let latency_buttons = action_buttons.clone();
    let on_latency = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        let log = latency_log.clone();
        let remote = latency_remote.value();
        let samples = input_usize(&latency_samples_input, 25, 3, 200);
        let node = latency_node.clone();
        let buttons = latency_buttons.clone();
        set_buttons_disabled(&buttons, true);
        spawn_local(async move {
            if let Err(error) = check_latency(node, remote, samples, log.clone()).await {
                log_line(&log, &format!("latency error: {error:?}"));
            }
            set_buttons_disabled(&buttons, false);
        });
    }) as Box<dyn FnMut(_)>);

    latency.add_event_listener_with_callback("click", on_latency.as_ref().unchecked_ref())?;
    on_latency.forget();

    let throughput_log = log.clone();
    let throughput_remote = remote.clone();
    let throughput_mib_input = throughput_mib.clone();
    let throughput_node = node.clone();
    let throughput_buttons = action_buttons.clone();
    let on_throughput = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        let log = throughput_log.clone();
        let remote = throughput_remote.value();
        let bytes = input_usize(&throughput_mib_input, 8, 1, 64) * 1024 * 1024;
        let node = throughput_node.clone();
        let buttons = throughput_buttons.clone();
        set_buttons_disabled(&buttons, true);
        spawn_local(async move {
            if let Err(error) = check_throughput(node, remote, bytes, log.clone()).await {
                log_line(&log, &format!("throughput error: {error:?}"));
            }
            set_buttons_disabled(&buttons, false);
        });
    }) as Box<dyn FnMut(_)>);

    throughput.add_event_listener_with_callback("click", on_throughput.as_ref().unchecked_ref())?;
    on_throughput.forget();
    Ok(())
}

async fn accept_connections(node: BrowserWebRtcNode, log: HtmlPreElement) -> Result<(), JsValue> {
    let acceptor = node.accept(ALPN).await?;
    loop {
        let Some(connection) = acceptor.accept().await? else {
            return Ok(());
        };
        let log = log.clone();
        spawn_local(async move {
            if let Err(error) = answer_requests(connection, log.clone()).await {
                log_line(&log, &format!("request error: {error:?}"));
            }
        });
    }
}

async fn answer_requests(
    connection: iroh_webrtc_transport::browser::BrowserWebRtcConnection,
    log: HtmlPreElement,
) -> Result<(), JsValue> {
    loop {
        let stream = match connection.accept_bi().await {
            Ok(stream) => stream,
            Err(_) => return Ok(()),
        };
        answer_stream(stream, connection.remote_endpoint_id(), &log).await?;
    }
}

async fn answer_stream(
    stream: iroh_webrtc_transport::browser::BrowserWebRtcStream,
    remote_endpoint_id: &str,
    log: &HtmlPreElement,
) -> Result<(), JsValue> {
    let mut reader = FrameReader::new(stream.clone());
    while let Some(request) = reader.read_frame().await? {
        let response = answer_request(&request, remote_endpoint_id, log)?;
        send_frame(&stream, &response).await?;
    }
    stream.close_send().await?;
    Ok(())
}

fn answer_request(
    request: &[u8],
    remote_endpoint_id: &str,
    log: &HtmlPreElement,
) -> Result<Vec<u8>, JsValue> {
    if request == b"ping" {
        log_line(log, &format!("received ping from {remote_endpoint_id}"));
        return Ok(b"pong".to_vec());
    }

    if let Some(rest) = request.strip_prefix(b"latency:") {
        return Ok([b"latency-ok:".as_slice(), rest].concat());
    }

    if let Some(size) = parse_size_request(request, "throughput-download:")? {
        return Ok(vec![0; size]);
    }

    if let Some((header, payload)) = split_header_payload(request) {
        if let Some(size) = parse_size_request(header, "throughput-upload:")? {
            if payload.len() != size {
                return Err(JsValue::from_str(&format!(
                    "throughput upload expected {size} bytes, received {}",
                    payload.len()
                )));
            }
            return Ok(b"throughput-ok".to_vec());
        }
    }

    Err(JsValue::from_str(&format!(
        "unknown benchmark request: {}",
        String::from_utf8_lossy(request)
    )))
}

async fn request_response(
    connection: &iroh_webrtc_transport::browser::BrowserWebRtcConnection,
    request: &[u8],
) -> Result<Vec<u8>, JsValue> {
    let stream = connection.open_bi().await?;
    send_frame(&stream, request).await?;
    stream.close_send().await?;
    let mut reader = FrameReader::new(stream);
    reader
        .read_frame()
        .await?
        .ok_or_else(|| JsValue::from_str("request stream closed before response"))
}

async fn request_ping(
    connection: &iroh_webrtc_transport::browser::BrowserWebRtcConnection,
) -> Result<Vec<u8>, JsValue> {
    request_response(connection, b"ping").await
}

async fn check_latency(
    node: BrowserWebRtcNode,
    remote: String,
    samples: usize,
    log: HtmlPreElement,
) -> Result<(), JsValue> {
    let remote = remote.trim();
    if remote.is_empty() {
        log_line(&log, "enter a remote endpoint first");
        return Ok(());
    }

    log_line(
        &log,
        &format!("latency check: {samples} samples to {remote}"),
    );
    let stats = node
        .check_latency(remote, BENCHMARK_ALPN, samples, LATENCY_WARMUP_SAMPLES)
        .await?;
    log_line(
        &log,
        &format!(
            "latency: n={} avg={} p50={} p95={} min={} max={}",
            stats.samples,
            format_ms(stats.avg_ms),
            format_ms(stats.p50_ms),
            format_ms(stats.p95_ms),
            format_ms(stats.min_ms),
            format_ms(stats.max_ms)
        ),
    );
    Ok(())
}

async fn ping_remote(
    node: BrowserWebRtcNode,
    remote: String,
    log: HtmlPreElement,
) -> Result<(), JsValue> {
    let remote = remote.trim();
    if remote.is_empty() {
        log_line(&log, "enter a remote endpoint first");
        return Ok(());
    }

    log_line(&log, &format!("dialing {remote}"));
    let connection = node
        .dial(remote, ALPN, BrowserDialOptions::webrtc_only())
        .await?;
    let response = request_ping(&connection).await?;
    log_line(
        &log,
        &format!("received {}", String::from_utf8_lossy(&response)),
    );

    connection.close("ping complete").await?;
    Ok(())
}

async fn check_throughput(
    node: BrowserWebRtcNode,
    remote: String,
    bytes: usize,
    log: HtmlPreElement,
) -> Result<(), JsValue> {
    let remote = remote.trim();
    if remote.is_empty() {
        log_line(&log, "enter a remote endpoint first");
        return Ok(());
    }

    log_line(
        &log,
        &format!("throughput check: {} to {remote}", format_mib(bytes)),
    );
    let stats = node
        .check_throughput(remote, BENCHMARK_ALPN, bytes, THROUGHPUT_WARMUP_BYTES)
        .await?;
    log_line(
        &log,
        &format!(
            "throughput: upload {} in {} ({}/s), download {} in {} ({}/s)",
            format_mib(stats.bytes),
            format_ms(stats.upload_ms),
            format_mib_per_second(stats.bytes, stats.upload_ms),
            format_mib(stats.bytes),
            format_ms(stats.download_ms),
            format_mib_per_second(stats.bytes, stats.download_ms)
        ),
    );
    Ok(())
}

fn document() -> Result<Document, JsValue> {
    web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("browser document is unavailable"))
}

fn query_flag(name: &str) -> Result<bool, JsValue> {
    let search = web_sys::window()
        .ok_or_else(|| JsValue::from_str("window is unavailable"))?
        .location()
        .search()?;
    let query = search.strip_prefix('?').unwrap_or(&search);
    Ok(query.split('&').any(|part| {
        let mut fields = part.splitn(2, '=');
        let Some(key) = fields.next() else {
            return false;
        };
        if key != name {
            return false;
        }
        !matches!(fields.next(), Some("0" | "false" | "off" | "no"))
    }))
}

fn element<T: JsCast>(document: &Document, id: &str) -> Result<T, JsValue> {
    document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing #{id}")))?
        .dyn_into::<T>()
        .map_err(|_| JsValue::from_str(&format!("#{id} has the wrong element type")))
}

fn input_usize(input: &HtmlInputElement, default: usize, min: usize, max: usize) -> usize {
    input
        .value()
        .trim()
        .parse::<usize>()
        .unwrap_or(default)
        .clamp(min, max)
}

fn set_buttons_disabled(buttons: &[HtmlButtonElement], disabled: bool) {
    for button in buttons {
        button.set_disabled(disabled);
    }
}

async fn send_frame(
    stream: &iroh_webrtc_transport::browser::BrowserWebRtcStream,
    payload: &[u8],
) -> Result<(), JsValue> {
    if payload.len() > u32::MAX as usize {
        return Err(JsValue::from_str("benchmark frame is too large"));
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    stream.send_all(&frame).await
}

struct FrameReader {
    stream: iroh_webrtc_transport::browser::BrowserWebRtcStream,
    buffer: Vec<u8>,
    eof: bool,
}

impl FrameReader {
    fn new(stream: iroh_webrtc_transport::browser::BrowserWebRtcStream) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            eof: false,
        }
    }

    async fn read_frame(&mut self) -> Result<Option<Vec<u8>>, JsValue> {
        if !self.fill_until(4).await? {
            if self.buffer.is_empty() {
                return Ok(None);
            }
            return Err(JsValue::from_str(
                "stream ended in the middle of a frame header",
            ));
        }

        let len = u32::from_be_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;
        if len > MAX_BENCHMARK_FRAME_BYTES {
            return Err(JsValue::from_str("benchmark frame exceeds the demo limit"));
        }

        if !self.fill_until(4 + len).await? {
            return Err(JsValue::from_str(
                "stream ended in the middle of a frame body",
            ));
        }

        self.buffer.drain(..4);
        Ok(Some(self.buffer.drain(..len).collect()))
    }

    async fn fill_until(&mut self, len: usize) -> Result<bool, JsValue> {
        while self.buffer.len() < len {
            if self.eof {
                return Ok(false);
            }
            match self.stream.read_chunk().await? {
                Some(chunk) => self.buffer.extend_from_slice(&chunk),
                None => self.eof = true,
            }
        }
        Ok(true)
    }
}

fn split_header_payload(request: &[u8]) -> Option<(&[u8], &[u8])> {
    let index = request.iter().position(|byte| *byte == b'\n')?;
    Some((&request[..index], &request[index + 1..]))
}

fn parse_size_request(request: &[u8], prefix: &str) -> Result<Option<usize>, JsValue> {
    let Some(rest) = request.strip_prefix(prefix.as_bytes()) else {
        return Ok(None);
    };
    let value = std::str::from_utf8(rest)
        .map_err(|_| JsValue::from_str("benchmark size request is not valid UTF-8"))?;
    let size = value
        .parse::<usize>()
        .map_err(|_| JsValue::from_str("benchmark size request is not a number"))?;
    Ok(Some(size))
}

fn format_ms(ms: f64) -> String {
    format!("{ms:.2} ms")
}

fn format_mib(bytes: usize) -> String {
    format!("{:.2} MiB", bytes as f64 / 1024.0 / 1024.0)
}

fn format_mib_per_second(bytes: usize, elapsed_ms: f64) -> String {
    if elapsed_ms <= 0.0 {
        return "inf MiB".to_owned();
    }
    format!(
        "{:.2} MiB",
        (bytes as f64 / 1024.0 / 1024.0) / (elapsed_ms / 1000.0)
    )
}

fn render_endpoint_qr(container: &Element, endpoint_id: &str) -> Result<(), JsValue> {
    let code = QrCode::new(endpoint_id.as_bytes())
        .map_err(|error| JsValue::from_str(&format!("failed to encode endpoint QR: {error}")))?;
    let image = code
        .render()
        .min_dimensions(192, 192)
        .dark_color(svg::Color("#111827"))
        .light_color(svg::Color("#ffffff"))
        .build();
    container.set_inner_html(&image);
    Ok(())
}

fn log_line(log: &HtmlPreElement, line: &str) {
    let existing = log.text_content().unwrap_or_default();
    log.set_text_content(Some(&format!("{existing}{line}\n")));
}
