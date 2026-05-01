use std::{future::pending, str::FromStr, time::Instant};

use iroh::{
    EndpointAddr, EndpointId, SecretKey,
    endpoint::{Connection, RecvStream, SendStream},
};
use iroh_webrtc_transport::{NativeWebRtcIrohNode, WebRtcDialOptions, WebRtcNodeConfig};
use n0_future::task;

const ALPN: &[u8] = b"example/iroh-webrtc-rust-browser-ping-pong/1";
const WORKER_BENCHMARK_ALPN: &[u8] =
    b"example/iroh-webrtc-rust-browser-ping-pong/worker-benchmark/1";
const LATENCY_SAMPLES: usize = 25;
const LATENCY_WARMUPS: usize = 3;
const THROUGHPUT_BYTES: usize = 32 * 1024 * 1024;
const THROUGHPUT_WARMUP_BYTES: usize = 256 * 1024;
const MAX_BENCHMARK_FRAME_BYTES: usize = 128 * 1024 * 1024;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let remote = std::env::args().nth(1);

    let node =
        NativeWebRtcIrohNode::spawn(WebRtcNodeConfig::default(), SecretKey::generate()).await?;
    let endpoint_id = node.endpoint_id();
    let endpoint_addr = node.endpoint_addr();
    println!("local endpoint: {endpoint_id}");
    println!("local endpoint addr: {endpoint_addr:?}");
    println!("app alpn: {}", String::from_utf8_lossy(ALPN));
    println!(
        "benchmark alpn: {}",
        String::from_utf8_lossy(WORKER_BENCHMARK_ALPN)
    );

    let app_accept = spawn_app_accept_loop(node.clone());
    let benchmark_accept = spawn_benchmark_accept_loop(node.clone());

    if let Some(remote) = remote {
        let remote = EndpointId::from_str(&remote)?;
        run_checks(&node, EndpointAddr::from(remote)).await?;
        node.close().await;
        app_accept.abort();
        benchmark_accept.abort();
        return Ok(());
    }

    println!("ready for browser demo peers; pass a remote endpoint id to run native checks");
    pending::<()>().await;
    Ok(())
}

fn spawn_app_accept_loop(node: NativeWebRtcIrohNode) -> task::JoinHandle<()> {
    task::spawn(async move {
        loop {
            let connection = match node.accept(ALPN).await {
                Ok(connection) => connection,
                Err(err) => {
                    eprintln!("app accept error: {err:#}");
                    return;
                }
            };
            task::spawn(async move {
                if let Err(err) = answer_app_connection(connection).await {
                    eprintln!("app connection error: {err:#}");
                }
            });
        }
    })
}

fn spawn_benchmark_accept_loop(node: NativeWebRtcIrohNode) -> task::JoinHandle<()> {
    task::spawn(async move {
        loop {
            let connection = match node.accept(WORKER_BENCHMARK_ALPN).await {
                Ok(connection) => connection,
                Err(err) => {
                    eprintln!("benchmark accept error: {err:#}");
                    return;
                }
            };
            task::spawn(async move {
                if let Err(err) = answer_benchmark_connection(connection).await {
                    eprintln!("benchmark connection error: {err:#}");
                }
            });
        }
    })
}

async fn answer_app_connection(connection: Connection) -> anyhow::Result<()> {
    loop {
        let (mut send, mut recv) = match connection.accept_bi().await {
            Ok(stream) => stream,
            Err(_) => return Ok(()),
        };
        task::spawn(async move {
            if let Err(err) = answer_app_stream(&mut send, &mut recv).await {
                eprintln!("app stream error: {err:#}");
            }
        });
    }
}

async fn answer_app_stream(send: &mut SendStream, recv: &mut RecvStream) -> anyhow::Result<()> {
    while let Some(request) = read_optional_frame(recv).await? {
        let response = answer_request(&request)?;
        write_frame(send, &response).await?;
    }
    let _ = send.finish();
    Ok(())
}

fn answer_request(request: &[u8]) -> anyhow::Result<Vec<u8>> {
    if request == b"ping" {
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
                anyhow::bail!(
                    "throughput upload expected {size} bytes, received {}",
                    payload.len()
                );
            }
            return Ok(b"throughput-ok".to_vec());
        }
    }
    anyhow::bail!("unknown app request: {}", String::from_utf8_lossy(request))
}

async fn answer_benchmark_connection(connection: Connection) -> anyhow::Result<()> {
    loop {
        let (mut send, mut recv) = match connection.accept_bi().await {
            Ok(stream) => stream,
            Err(_) => return Ok(()),
        };
        task::spawn(async move {
            if let Err(err) = answer_benchmark_stream(&mut send, &mut recv).await {
                eprintln!("benchmark stream error: {err:#}");
            }
        });
    }
}

async fn answer_benchmark_stream(
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> anyhow::Result<()> {
    while let Some(request) = read_optional_frame(recv).await? {
        if let Some(rest) = request.strip_prefix(b"latency:") {
            write_frame(send, &[b"latency-ok:".as_slice(), rest].concat()).await?;
            continue;
        }
        if let Some(bytes) = parse_size_request(&request, "throughput-upload:")? {
            discard_bytes(recv, bytes).await?;
            write_frame(send, b"throughput-ok").await?;
            continue;
        }
        if let Some(bytes) = parse_size_request(&request, "throughput-download:")? {
            write_zero_bytes(send, bytes).await?;
            continue;
        }
        anyhow::bail!(
            "unknown benchmark request: {}",
            String::from_utf8_lossy(&request)
        );
    }
    let _ = send.finish();
    Ok(())
}

async fn run_checks(node: &NativeWebRtcIrohNode, remote: EndpointAddr) -> anyhow::Result<()> {
    println!("dialing {}", remote.id);
    let connection = node
        .dial(remote.clone(), ALPN, WebRtcDialOptions::webrtc_only())
        .await?;
    let response = request_response(&connection, b"ping").await?;
    println!("ping: received {}", String::from_utf8_lossy(&response));
    connection.close(0u32.into(), b"native ping complete");

    let connection = node
        .dial(
            remote,
            WORKER_BENCHMARK_ALPN,
            WebRtcDialOptions::webrtc_only(),
        )
        .await?;
    let latency = check_latency(&connection, LATENCY_SAMPLES, LATENCY_WARMUPS).await?;
    println!(
        "latency: n={} avg={} p50={} p95={} min={} max={}",
        latency.samples,
        format_ms(latency.avg_ms),
        format_ms(latency.p50_ms),
        format_ms(latency.p95_ms),
        format_ms(latency.min_ms),
        format_ms(latency.max_ms)
    );

    let throughput =
        check_throughput(&connection, THROUGHPUT_BYTES, THROUGHPUT_WARMUP_BYTES).await?;
    println!(
        "throughput: upload {} in {} ({}/s), download {} in {} ({}/s)",
        format_mib(throughput.bytes),
        format_ms(throughput.upload_ms),
        format_mib_per_second(throughput.bytes, throughput.upload_ms),
        format_mib(throughput.bytes),
        format_ms(throughput.download_ms),
        format_mib_per_second(throughput.bytes, throughput.download_ms)
    );
    connection.close(0u32.into(), b"native benchmark complete");
    Ok(())
}

async fn request_response(connection: &Connection, request: &[u8]) -> anyhow::Result<Vec<u8>> {
    let (mut send, mut recv) = connection.open_bi().await?;
    write_frame(&mut send, request).await?;
    send.finish()?;
    read_frame(&mut recv).await
}

async fn check_latency(
    connection: &Connection,
    samples: usize,
    warmups: usize,
) -> anyhow::Result<LatencyStats> {
    let (mut send, mut recv) = connection.open_bi().await?;
    for sample in 0..warmups {
        let request = format!("latency:warmup-{sample}");
        let expected = format!("latency-ok:warmup-{sample}");
        write_frame(&mut send, request.as_bytes()).await?;
        let response = read_frame(&mut recv).await?;
        if response != expected.as_bytes() {
            anyhow::bail!("latency warmup returned an invalid response");
        }
    }

    let mut elapsed = Vec::with_capacity(samples);
    for sample in 0..samples {
        let request = format!("latency:{sample}");
        let expected = format!("latency-ok:{sample}");
        let started = Instant::now();
        write_frame(&mut send, request.as_bytes()).await?;
        let response = read_frame(&mut recv).await?;
        if response != expected.as_bytes() {
            anyhow::bail!("latency sample returned an invalid response");
        }
        elapsed.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let _ = send.finish();
    LatencyStats::from_samples(elapsed)
}

async fn check_throughput(
    connection: &Connection,
    bytes: usize,
    warmup_bytes: usize,
) -> anyhow::Result<ThroughputStats> {
    let (mut send, mut recv) = connection.open_bi().await?;
    run_upload_round(&mut send, &mut recv, warmup_bytes).await?;
    run_download_round(&mut send, &mut recv, warmup_bytes).await?;

    let started = Instant::now();
    run_upload_round(&mut send, &mut recv, bytes).await?;
    let upload_ms = started.elapsed().as_secs_f64() * 1000.0;

    let started = Instant::now();
    run_download_round(&mut send, &mut recv, bytes).await?;
    let download_ms = started.elapsed().as_secs_f64() * 1000.0;

    let _ = send.finish();
    Ok(ThroughputStats {
        bytes,
        upload_ms,
        download_ms,
    })
}

async fn run_upload_round(
    send: &mut SendStream,
    recv: &mut RecvStream,
    bytes: usize,
) -> anyhow::Result<()> {
    write_frame(send, format!("throughput-upload:{bytes}").as_bytes()).await?;
    write_zero_bytes(send, bytes).await?;
    let response = read_frame(recv).await?;
    if response != b"throughput-ok" {
        anyhow::bail!("throughput upload returned an invalid ack");
    }
    Ok(())
}

async fn run_download_round(
    send: &mut SendStream,
    recv: &mut RecvStream,
    bytes: usize,
) -> anyhow::Result<()> {
    write_frame(send, format!("throughput-download:{bytes}").as_bytes()).await?;
    discard_bytes(recv, bytes).await
}

async fn write_frame(send: &mut SendStream, payload: &[u8]) -> anyhow::Result<()> {
    if payload.len() > u32::MAX as usize {
        anyhow::bail!("benchmark frame is too large");
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    send.write_all(&frame).await?;
    Ok(())
}

async fn read_frame(recv: &mut RecvStream) -> anyhow::Result<Vec<u8>> {
    read_optional_frame(recv)
        .await?
        .ok_or_else(|| anyhow::anyhow!("stream closed before response"))
}

async fn read_optional_frame(recv: &mut RecvStream) -> anyhow::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    if !read_exact(recv, &mut len).await? {
        return Ok(None);
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_BENCHMARK_FRAME_BYTES {
        anyhow::bail!("benchmark frame exceeds the demo limit");
    }
    let mut payload = vec![0; len];
    if !read_exact(recv, &mut payload).await? {
        anyhow::bail!("stream closed in the middle of a frame");
    }
    Ok(Some(payload))
}

async fn read_exact(recv: &mut RecvStream, mut out: &mut [u8]) -> anyhow::Result<bool> {
    while !out.is_empty() {
        let Some(len) = recv.read(out).await? else {
            return Ok(false);
        };
        if len == 0 {
            return Ok(false);
        }
        let (_, rest) = out.split_at_mut(len);
        out = rest;
    }
    Ok(true)
}

async fn write_zero_bytes(send: &mut SendStream, mut len: usize) -> anyhow::Result<()> {
    const ZERO_CHUNK: &[u8; 64 * 1024] = &[0; 64 * 1024];
    while len > 0 {
        let chunk_len = len.min(ZERO_CHUNK.len());
        send.write_all(&ZERO_CHUNK[..chunk_len]).await?;
        len -= chunk_len;
    }
    Ok(())
}

async fn discard_bytes(recv: &mut RecvStream, mut len: usize) -> anyhow::Result<()> {
    let mut buffer = vec![0; 64 * 1024];
    while len > 0 {
        let chunk_len = len.min(buffer.len());
        if !read_exact(recv, &mut buffer[..chunk_len]).await? {
            anyhow::bail!("stream closed before all bytes were received");
        }
        len -= chunk_len;
    }
    Ok(())
}

fn split_header_payload(request: &[u8]) -> Option<(&[u8], &[u8])> {
    let index = request.iter().position(|byte| *byte == b'\n')?;
    Some((&request[..index], &request[index + 1..]))
}

fn parse_size_request(request: &[u8], prefix: &str) -> anyhow::Result<Option<usize>> {
    let Some(rest) = request.strip_prefix(prefix.as_bytes()) else {
        return Ok(None);
    };
    let value = std::str::from_utf8(rest)?;
    Ok(Some(value.parse()?))
}

#[derive(Debug)]
struct LatencyStats {
    samples: usize,
    avg_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

impl LatencyStats {
    fn from_samples(mut samples: Vec<f64>) -> anyhow::Result<Self> {
        if samples.is_empty() {
            anyhow::bail!("latency stats require at least one sample");
        }
        samples.sort_by(|a, b| a.total_cmp(b));
        let sum = samples.iter().sum::<f64>();
        Ok(Self {
            samples: samples.len(),
            avg_ms: sum / samples.len() as f64,
            p50_ms: percentile(&samples, 0.50),
            p95_ms: percentile(&samples, 0.95),
            min_ms: samples[0],
            max_ms: samples[samples.len() - 1],
        })
    }
}

#[derive(Debug)]
struct ThroughputStats {
    bytes: usize,
    upload_ms: f64,
    download_ms: f64,
}

fn percentile(sorted_samples: &[f64], percentile: f64) -> f64 {
    let index = ((sorted_samples.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted_samples.len() - 1);
    sorted_samples[index]
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
