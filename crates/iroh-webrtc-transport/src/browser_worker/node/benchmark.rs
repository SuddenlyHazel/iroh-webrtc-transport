use super::*;

const BENCHMARK_MAX_FRAME_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerBenchmarkEchoOpenResult {
    pub(in crate::browser_worker) opened: bool,
    pub(in crate::browser_worker) alpn: String,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerBenchmarkLatencyResult {
    pub(in crate::browser_worker) samples: usize,
    pub(in crate::browser_worker) avg_ms: f64,
    pub(in crate::browser_worker) p50_ms: f64,
    pub(in crate::browser_worker) p95_ms: f64,
    pub(in crate::browser_worker) min_ms: f64,
    pub(in crate::browser_worker) max_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::browser_worker) struct WorkerBenchmarkThroughputResult {
    pub(in crate::browser_worker) bytes: usize,
    pub(in crate::browser_worker) upload_ms: f64,
    pub(in crate::browser_worker) download_ms: f64,
    pub(in crate::browser_worker) transport_stats: crate::core::hub::WebRtcTransportStatsSnapshot,
}

impl BrowserWorkerNode {
    pub(in crate::browser_worker) fn open_benchmark_echo(
        &self,
        alpn: impl Into<String>,
    ) -> BrowserWorkerResult<WorkerBenchmarkEchoOpenResult> {
        let alpn = validate_alpn(alpn.into())?;
        {
            let inner = self
                .inner
                .lock()
                .expect("browser worker node mutex poisoned");
            if inner.closed {
                return Err(BrowserWorkerError::closed());
            }
            if !inner.benchmark_echo_alpns.contains(&alpn) {
                return Err(BrowserWorkerError::new(
                    BrowserWorkerErrorCode::UnsupportedAlpn,
                    format!("ALPN {alpn:?} was not registered as a benchmark echo protocol"),
                ));
            }
        }
        Ok(WorkerBenchmarkEchoOpenResult {
            opened: false,
            alpn,
        })
    }

    pub(in crate::browser_worker) async fn benchmark_latency_on_connection(
        &self,
        connection_key: WorkerConnectionKey,
        samples: usize,
        warmups: usize,
    ) -> BrowserWorkerResult<WorkerBenchmarkLatencyResult> {
        if samples == 0 {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "benchmark latency requires at least one sample",
            ));
        }
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::benchmark",
            worker_now_ms = benchmark_now_ms(),
            connection_key = connection_key.0,
            samples,
            warmups,
            "benchmark latency opening stream"
        );
        let connection = self.iroh_connection(connection_key)?;
        trace_iroh_connection_paths(
            "benchmark latency before opening stream",
            Some(connection_key.0),
            &connection,
        );
        let (mut send, mut recv) = connection.open_bi().await.map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("failed to open benchmark stream: {err}"),
            )
        })?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::benchmark",
            worker_now_ms = benchmark_now_ms(),
            connection_key = connection_key.0,
            "benchmark latency stream open"
        );
        trace_iroh_connection_paths(
            "benchmark latency stream open",
            Some(connection_key.0),
            &connection,
        );

        for sample in 0..warmups {
            let request = format!("latency:warmup-{sample}");
            let expected = format!("latency-ok:warmup-{sample}");
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::benchmark",
                worker_now_ms = benchmark_now_ms(),
                connection_key = connection_key.0,
                warmup = sample,
                "benchmark latency warmup send start"
            );
            write_benchmark_frame(&mut send, request.as_bytes()).await?;
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::benchmark",
                worker_now_ms = benchmark_now_ms(),
                connection_key = connection_key.0,
                warmup = sample,
                "benchmark latency warmup send end"
            );
            let response = read_benchmark_frame(&mut recv).await?;
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::benchmark",
                worker_now_ms = benchmark_now_ms(),
                connection_key = connection_key.0,
                warmup = sample,
                response_bytes = response.len(),
                "benchmark latency warmup read end"
            );
            if response != expected.as_bytes() {
                return Err(BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    "latency warmup returned an invalid response",
                ));
            }
        }

        trace_iroh_connection_paths(
            "benchmark latency after warmups",
            Some(connection_key.0),
            &connection,
        );

        let mut timings = Vec::with_capacity(samples);
        for sample in 0..samples {
            let request = format!("latency:{sample}");
            let expected = format!("latency-ok:{sample}");
            let started_ms = benchmark_now_ms();
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::benchmark",
                worker_now_ms = started_ms,
                connection_key = connection_key.0,
                sample,
                "benchmark latency sample send start"
            );
            write_benchmark_frame(&mut send, request.as_bytes()).await?;
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::benchmark",
                worker_now_ms = benchmark_now_ms(),
                connection_key = connection_key.0,
                sample,
                "benchmark latency sample send end"
            );
            let response = read_benchmark_frame(&mut recv).await?;
            let elapsed_ms = benchmark_now_ms() - started_ms;
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::benchmark",
                worker_now_ms = benchmark_now_ms(),
                connection_key = connection_key.0,
                sample,
                response_bytes = response.len(),
                elapsed_ms,
                "benchmark latency sample read end"
            );
            if response != expected.as_bytes() {
                return Err(BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    "latency sample returned an invalid response",
                ));
            }
            if sample == 0 {
                trace_iroh_connection_paths(
                    "benchmark latency after first sample",
                    Some(connection_key.0),
                    &connection,
                );
            }
            timings.push(elapsed_ms);
        }

        let _ = send.finish();
        let result = WorkerBenchmarkLatencyResult::from_samples(timings);
        if let Ok(stats) = result {
            trace_iroh_connection_paths(
                "benchmark latency complete",
                Some(connection_key.0),
                &connection,
            );
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::benchmark",
                worker_now_ms = benchmark_now_ms(),
                connection_key = connection_key.0,
                samples = stats.samples,
                avg_ms = stats.avg_ms,
                p50_ms = stats.p50_ms,
                p95_ms = stats.p95_ms,
                min_ms = stats.min_ms,
                max_ms = stats.max_ms,
                "benchmark latency complete"
            );
        }
        let _ = self.connection_close(connection_key, Some("benchmark latency complete".into()));
        result
    }

    pub(in crate::browser_worker) async fn benchmark_throughput_on_connection(
        &self,
        connection_key: WorkerConnectionKey,
        bytes: usize,
        warmup_bytes: usize,
    ) -> BrowserWorkerResult<WorkerBenchmarkThroughputResult> {
        if bytes == 0 {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "benchmark throughput requires at least one byte",
            ));
        }
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::benchmark",
            worker_now_ms = benchmark_now_ms(),
            connection_key = connection_key.0,
            bytes,
            warmup_bytes,
            "benchmark throughput opening stream"
        );
        let connection = self.iroh_connection(connection_key)?;
        trace_iroh_connection_paths(
            "benchmark throughput before opening stream",
            Some(connection_key.0),
            &connection,
        );
        let (mut send, mut recv) = connection.open_bi().await.map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("failed to open benchmark stream: {err}"),
            )
        })?;
        trace_iroh_connection_paths(
            "benchmark throughput stream open",
            Some(connection_key.0),
            &connection,
        );
        let hub = self.session_hub();
        let stats_before = hub.stats_snapshot();

        let warmup_bytes = bytes.min(warmup_bytes);
        if warmup_bytes > 0 {
            let warmup_stats_before = hub.stats_snapshot();
            let warmup_start = benchmark_now_ms();
            run_upload_benchmark_round(&mut send, &mut recv, warmup_bytes).await?;
            run_download_benchmark_round(&mut send, &mut recv, warmup_bytes).await?;
            let warmup_ms = benchmark_now_ms() - warmup_start;
            let warmup_stats = hub.stats_snapshot().delta_since(warmup_stats_before);
            log_throughput_phase_stats(
                connection_key,
                "warmup",
                warmup_ms,
                warmup_bytes * 2,
                &warmup_stats,
            );
            trace_iroh_connection_paths(
                "benchmark throughput after warmup",
                Some(connection_key.0),
                &connection,
            );
        }

        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::benchmark",
            worker_now_ms = benchmark_now_ms(),
            connection_key = connection_key.0,
            bytes,
            "benchmark throughput upload start"
        );
        let upload_stats_before = hub.stats_snapshot();
        let upload_start = benchmark_now_ms();
        run_upload_benchmark_round(&mut send, &mut recv, bytes).await?;
        let upload_ms = benchmark_now_ms() - upload_start;
        let upload_stats = hub.stats_snapshot().delta_since(upload_stats_before);
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::benchmark",
            worker_now_ms = benchmark_now_ms(),
            connection_key = connection_key.0,
            bytes,
            upload_ms,
            "benchmark throughput upload complete"
        );
        log_throughput_phase_stats(connection_key, "upload", upload_ms, bytes, &upload_stats);
        trace_iroh_connection_paths(
            "benchmark throughput after upload",
            Some(connection_key.0),
            &connection,
        );

        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::benchmark",
            worker_now_ms = benchmark_now_ms(),
            connection_key = connection_key.0,
            bytes,
            "benchmark throughput download start"
        );
        let download_stats_before = hub.stats_snapshot();
        let download_start = benchmark_now_ms();
        run_download_benchmark_round(&mut send, &mut recv, bytes).await?;
        let download_ms = benchmark_now_ms() - download_start;
        let download_stats = hub.stats_snapshot().delta_since(download_stats_before);
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::benchmark",
            worker_now_ms = benchmark_now_ms(),
            connection_key = connection_key.0,
            bytes,
            download_ms,
            "benchmark throughput download complete"
        );
        log_throughput_phase_stats(
            connection_key,
            "download",
            download_ms,
            bytes,
            &download_stats,
        );
        trace_iroh_connection_paths(
            "benchmark throughput after download",
            Some(connection_key.0),
            &connection,
        );

        let _ = send.finish();
        trace_iroh_connection_paths(
            "benchmark throughput complete",
            Some(connection_key.0),
            &connection,
        );
        let transport_stats = hub.stats_snapshot().delta_since(stats_before);
        log_throughput_transport_stats(connection_key, &transport_stats);
        let _ = self.connection_close(connection_key, Some("benchmark throughput complete".into()));
        Ok(WorkerBenchmarkThroughputResult {
            bytes,
            upload_ms,
            download_ms,
            transport_stats,
        })
    }

    pub(super) async fn run_benchmark_echo_connection(
        &self,
        connection_key: WorkerConnectionKey,
    ) -> BrowserWorkerResult<()> {
        let connection = self.iroh_connection(connection_key)?;
        trace_iroh_connection_paths(
            "benchmark echo connection start",
            Some(connection_key.0),
            &connection,
        );
        loop {
            let (mut send, mut recv) = match connection.accept_bi().await {
                Ok(stream) => stream,
                Err(_) => return Ok(()),
            };
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::benchmark",
                worker_now_ms = benchmark_now_ms(),
                connection_key = connection_key.0,
                "benchmark echo accepted stream"
            );
            trace_iroh_connection_paths(
                "benchmark echo accepted stream",
                Some(connection_key.0),
                &connection,
            );
            let connection = connection.clone();
            let hub = self.session_hub();
            n0_future::task::spawn(async move {
                if let Err(err) =
                    echo_benchmark_stream(connection_key, connection, hub, &mut send, &mut recv)
                        .await
                {
                    tracing::debug!(
                        target: "iroh_webrtc_transport::browser_worker::benchmark",
                        %err,
                        "benchmark echo stream failed"
                    );
                }
            });
        }
    }
}

fn log_throughput_transport_stats(
    connection_key: WorkerConnectionKey,
    stats: &crate::core::hub::WebRtcTransportStatsSnapshot,
) {
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_worker::benchmark",
        worker_now_ms = benchmark_now_ms(),
        connection_key = connection_key.0,
        sender_poll_send_calls = stats.sender_poll_send_calls,
        sender_poll_send_gap_samples = stats.sender_poll_send_gap_samples,
        sender_poll_send_gap_ms_total = stats.sender_poll_send_gap_ms_total,
        sender_poll_send_gap_ms_max = stats.sender_poll_send_gap_ms_max,
        sender_poll_send_gap_over_1ms = stats.sender_poll_send_gap_over_1ms,
        sender_poll_send_gap_over_4ms = stats.sender_poll_send_gap_over_4ms,
        sender_poll_send_gap_over_16ms = stats.sender_poll_send_gap_over_16ms,
        sender_transmits = stats.sender_transmits,
        sender_payload_bytes = stats.sender_payload_bytes,
        sender_frame_bytes = stats.sender_frame_bytes,
        sender_transmit_datagrams = stats.sender_transmit_datagrams,
        sender_transmit_datagrams_max = stats.sender_transmit_datagrams_max,
        sender_transmits_single_datagram = stats.sender_transmits_single_datagram,
        sender_transmits_segmented = stats.sender_transmits_segmented,
        sender_transmits_datagrams_2_to_4 = stats.sender_transmits_datagrams_2_to_4,
        sender_transmits_datagrams_5_to_9 = stats.sender_transmits_datagrams_5_to_9,
        sender_transmits_full_batch = stats.sender_transmits_full_batch,
        sender_transmit_payload_max = stats.sender_transmit_payload_max,
        sender_inter_transmit_gap_samples = stats.sender_inter_transmit_gap_samples,
        sender_inter_transmit_gap_ms_total = stats.sender_inter_transmit_gap_ms_total,
        sender_inter_transmit_gap_ms_max = stats.sender_inter_transmit_gap_ms_max,
        sender_inter_transmit_gap_over_1ms = stats.sender_inter_transmit_gap_over_1ms,
        sender_inter_transmit_gap_over_4ms = stats.sender_inter_transmit_gap_over_4ms,
        sender_inter_transmit_gap_over_16ms = stats.sender_inter_transmit_gap_over_16ms,
        sender_encode_failures = stats.sender_encode_failures,
        sender_queue_failures = stats.sender_queue_failures,
        hub_recv_push_gap_samples = stats.hub_recv_push_gap_samples,
        hub_recv_push_gap_ms_total = stats.hub_recv_push_gap_ms_total,
        hub_recv_push_gap_ms_max = stats.hub_recv_push_gap_ms_max,
        hub_recv_push_gap_over_1ms = stats.hub_recv_push_gap_over_1ms,
        hub_recv_push_gap_over_4ms = stats.hub_recv_push_gap_over_4ms,
        hub_recv_push_gap_over_16ms = stats.hub_recv_push_gap_over_16ms,
        hub_recv_wake_to_poll_gap_samples = stats.hub_recv_wake_to_poll_gap_samples,
        hub_recv_wake_to_poll_gap_ms_total = stats.hub_recv_wake_to_poll_gap_ms_total,
        hub_recv_wake_to_poll_gap_ms_max = stats.hub_recv_wake_to_poll_gap_ms_max,
        hub_recv_wake_to_poll_gap_over_1ms = stats.hub_recv_wake_to_poll_gap_over_1ms,
        hub_recv_wake_to_poll_gap_over_4ms = stats.hub_recv_wake_to_poll_gap_over_4ms,
        hub_recv_wake_to_poll_gap_over_16ms = stats.hub_recv_wake_to_poll_gap_over_16ms,
        hub_recv_pushes = stats.hub_recv_pushes,
        hub_recv_waker_sets = stats.hub_recv_waker_sets,
        hub_recv_wakes = stats.hub_recv_wakes,
        hub_outbound_push_gap_samples = stats.hub_outbound_push_gap_samples,
        hub_outbound_push_gap_ms_total = stats.hub_outbound_push_gap_ms_total,
        hub_outbound_push_gap_ms_max = stats.hub_outbound_push_gap_ms_max,
        hub_outbound_push_gap_over_1ms = stats.hub_outbound_push_gap_over_1ms,
        hub_outbound_push_gap_over_4ms = stats.hub_outbound_push_gap_over_4ms,
        hub_outbound_push_gap_over_16ms = stats.hub_outbound_push_gap_over_16ms,
        hub_outbound_pushes = stats.hub_outbound_pushes,
        hub_outbound_bytes = stats.hub_outbound_bytes,
        hub_outbound_depth_max = stats.hub_outbound_depth_max,
        hub_outbound_full = stats.hub_outbound_full,
        hub_outbound_waker_sets = stats.hub_outbound_waker_sets,
        hub_outbound_wakes = stats.hub_outbound_wakes,
        hub_pop_matching_calls = stats.hub_pop_matching_calls,
        hub_pop_matching_hits = stats.hub_pop_matching_hits,
        hub_pop_matching_pending = stats.hub_pop_matching_pending,
        hub_pop_matching_scan_total = stats.hub_pop_matching_scan_total,
        hub_pop_matching_scan_max = stats.hub_pop_matching_scan_max,
        data_channel_rx_message_gap_samples = stats.data_channel_rx_message_gap_samples,
        data_channel_rx_message_gap_ms_total = stats.data_channel_rx_message_gap_ms_total,
        data_channel_rx_message_gap_ms_max = stats.data_channel_rx_message_gap_ms_max,
        data_channel_rx_message_gap_over_1ms = stats.data_channel_rx_message_gap_over_1ms,
        data_channel_rx_message_gap_over_4ms = stats.data_channel_rx_message_gap_over_4ms,
        data_channel_rx_message_gap_over_16ms = stats.data_channel_rx_message_gap_over_16ms,
        data_channel_rx_messages = stats.data_channel_rx_messages,
        data_channel_rx_frame_bytes = stats.data_channel_rx_frame_bytes,
        data_channel_rx_payload_bytes = stats.data_channel_rx_payload_bytes,
        data_channel_rx_invalid_frames = stats.data_channel_rx_invalid_frames,
        data_channel_rx_wrong_session = stats.data_channel_rx_wrong_session,
        data_channel_rx_enqueue_failures = stats.data_channel_rx_enqueue_failures,
        data_channel_pump_pop_gap_samples = stats.data_channel_pump_pop_gap_samples,
        data_channel_pump_pop_gap_ms_total = stats.data_channel_pump_pop_gap_ms_total,
        data_channel_pump_pop_gap_ms_max = stats.data_channel_pump_pop_gap_ms_max,
        data_channel_pump_pop_gap_over_1ms = stats.data_channel_pump_pop_gap_over_1ms,
        data_channel_pump_pop_gap_over_4ms = stats.data_channel_pump_pop_gap_over_4ms,
        data_channel_pump_pop_gap_over_16ms = stats.data_channel_pump_pop_gap_over_16ms,
        data_channel_pump_pops = stats.data_channel_pump_pops,
        data_channel_send_gap_samples = stats.data_channel_send_gap_samples,
        data_channel_send_gap_ms_total = stats.data_channel_send_gap_ms_total,
        data_channel_send_gap_ms_max = stats.data_channel_send_gap_ms_max,
        data_channel_send_gap_over_1ms = stats.data_channel_send_gap_over_1ms,
        data_channel_send_gap_over_4ms = stats.data_channel_send_gap_over_4ms,
        data_channel_send_gap_over_16ms = stats.data_channel_send_gap_over_16ms,
        data_channel_sent_messages = stats.data_channel_sent_messages,
        data_channel_sent_bytes = stats.data_channel_sent_bytes,
        data_channel_buffered_amount_before_max = stats.data_channel_buffered_amount_before_max,
        data_channel_buffered_amount_max = stats.data_channel_buffered_amount_max,
        data_channel_send_failures = stats.data_channel_send_failures,
        endpoint_poll_recv_call_gap_samples = stats.endpoint_poll_recv_call_gap_samples,
        endpoint_poll_recv_call_gap_ms_total = stats.endpoint_poll_recv_call_gap_ms_total,
        endpoint_poll_recv_call_gap_ms_max = stats.endpoint_poll_recv_call_gap_ms_max,
        endpoint_poll_recv_call_gap_over_1ms = stats.endpoint_poll_recv_call_gap_over_1ms,
        endpoint_poll_recv_call_gap_over_4ms = stats.endpoint_poll_recv_call_gap_over_4ms,
        endpoint_poll_recv_call_gap_over_16ms = stats.endpoint_poll_recv_call_gap_over_16ms,
        endpoint_poll_recv_calls = stats.endpoint_poll_recv_calls,
        endpoint_poll_recv_pending = stats.endpoint_poll_recv_pending,
        endpoint_poll_recv_ready = stats.endpoint_poll_recv_ready,
        endpoint_poll_recv_ready_packets = stats.endpoint_poll_recv_ready_packets,
        endpoint_poll_recv_ready_packets_max = stats.endpoint_poll_recv_ready_packets_max,
        endpoint_poll_recv_partial_pending = stats.endpoint_poll_recv_partial_pending,
        endpoint_poll_recv_full = stats.endpoint_poll_recv_full,
        endpoint_poll_recv_pending_frame_continuations =
            stats.endpoint_poll_recv_pending_frame_continuations,
        endpoint_delivered_packets = stats.endpoint_delivered_packets,
        endpoint_delivered_payload_bytes = stats.endpoint_delivered_payload_bytes,
        endpoint_delivered_payload_max = stats.endpoint_delivered_payload_max,
        "benchmark throughput transport stats"
    );
    tracing::debug!(
        target: "iroh_webrtc_transport::browser_worker::benchmark",
        worker_now_ms = benchmark_now_ms(),
        connection_key = connection_key.0,
        hub_outbound_queue_delay_samples = stats.hub_outbound_queue_delay_samples,
        hub_outbound_queue_delay_ms_total = stats.hub_outbound_queue_delay_ms_total,
        hub_outbound_queue_delay_ms_max = stats.hub_outbound_queue_delay_ms_max,
        hub_outbound_queue_delay_over_1ms = stats.hub_outbound_queue_delay_over_1ms,
        hub_outbound_queue_delay_over_4ms = stats.hub_outbound_queue_delay_over_4ms,
        hub_outbound_queue_delay_over_16ms = stats.hub_outbound_queue_delay_over_16ms,
        data_channel_buffered_amount_samples = stats.data_channel_buffered_amount_samples,
        data_channel_buffered_amount_before_total =
            stats.data_channel_buffered_amount_before_total,
        data_channel_buffered_amount_after_total = stats.data_channel_buffered_amount_after_total,
        data_channel_buffered_amount_before_over_256k =
            stats.data_channel_buffered_amount_before_over_256k,
        data_channel_buffered_amount_before_over_512k =
            stats.data_channel_buffered_amount_before_over_512k,
        data_channel_buffered_amount_before_over_1m =
            stats.data_channel_buffered_amount_before_over_1m,
        data_channel_buffered_amount_after_over_256k =
            stats.data_channel_buffered_amount_after_over_256k,
        data_channel_buffered_amount_after_over_512k =
            stats.data_channel_buffered_amount_after_over_512k,
        data_channel_buffered_amount_after_over_1m =
            stats.data_channel_buffered_amount_after_over_1m,
        "benchmark throughput transport queue stats"
    );
}

fn log_throughput_phase_stats(
    connection_key: WorkerConnectionKey,
    phase: &'static str,
    duration_ms: f64,
    bytes: usize,
    stats: &crate::core::hub::WebRtcTransportStatsSnapshot,
) {
    tracing::debug!(
        target: "iroh_webrtc_transport::browser_worker::benchmark",
        worker_now_ms = benchmark_now_ms(),
        connection_key = connection_key.0,
        phase,
        duration_ms,
        bytes,
        sender_poll_send_calls = stats.sender_poll_send_calls,
        sender_poll_send_gap_ms_total = stats.sender_poll_send_gap_ms_total,
        sender_poll_send_gap_ms_max = stats.sender_poll_send_gap_ms_max,
        sender_transmits = stats.sender_transmits,
        sender_payload_bytes = stats.sender_payload_bytes,
        sender_transmit_datagrams = stats.sender_transmit_datagrams,
        sender_transmits_single_datagram = stats.sender_transmits_single_datagram,
        sender_transmits_full_batch = stats.sender_transmits_full_batch,
        sender_transmit_payload_max = stats.sender_transmit_payload_max,
        hub_outbound_depth_max = stats.hub_outbound_depth_max,
        hub_outbound_full = stats.hub_outbound_full,
        hub_outbound_queue_delay_ms_total = stats.hub_outbound_queue_delay_ms_total,
        hub_outbound_queue_delay_ms_max = stats.hub_outbound_queue_delay_ms_max,
        data_channel_sent_messages = stats.data_channel_sent_messages,
        data_channel_sent_bytes = stats.data_channel_sent_bytes,
        data_channel_buffered_amount_after_total = stats.data_channel_buffered_amount_after_total,
        data_channel_buffered_amount_max = stats.data_channel_buffered_amount_max,
        data_channel_buffered_amount_after_over_256k =
            stats.data_channel_buffered_amount_after_over_256k,
        data_channel_buffered_amount_after_over_512k =
            stats.data_channel_buffered_amount_after_over_512k,
        data_channel_buffered_amount_after_over_1m =
            stats.data_channel_buffered_amount_after_over_1m,
        data_channel_rx_messages = stats.data_channel_rx_messages,
        endpoint_delivered_packets = stats.endpoint_delivered_packets,
        endpoint_poll_recv_pending = stats.endpoint_poll_recv_pending,
        "benchmark throughput phase stats"
    );
}

impl WorkerBenchmarkLatencyResult {
    fn from_samples(mut samples: Vec<f64>) -> BrowserWorkerResult<Self> {
        if samples.is_empty() {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "benchmark latency stats require at least one sample",
            ));
        }
        samples.sort_by(|a, b| a.total_cmp(b));
        let sum = samples.iter().sum::<f64>();
        Ok(Self {
            samples: samples.len(),
            avg_ms: sum / samples.len() as f64,
            p50_ms: benchmark_percentile(&samples, 0.50),
            p95_ms: benchmark_percentile(&samples, 0.95),
            min_ms: samples[0],
            max_ms: samples[samples.len() - 1],
        })
    }
}

async fn echo_benchmark_stream(
    connection_key: WorkerConnectionKey,
    connection: Connection,
    hub: crate::core::hub::SessionHub,
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> BrowserWorkerResult<()> {
    while let Some(request) = read_optional_benchmark_frame(recv).await? {
        let response = if let Some(rest) = request.strip_prefix(b"latency:") {
            [b"latency-ok:".as_slice(), rest].concat()
        } else if let Some(bytes) = parse_benchmark_size_request(&request, "throughput-upload:")? {
            let stats_before = hub.stats_snapshot();
            let started_ms = benchmark_now_ms();
            discard_benchmark_bytes(recv, bytes).await?;
            let duration_ms = benchmark_now_ms() - started_ms;
            let stats = hub.stats_snapshot().delta_since(stats_before);
            log_throughput_phase_stats(
                connection_key,
                "echo-upload-recv",
                duration_ms,
                bytes,
                &stats,
            );
            trace_iroh_connection_paths(
                "benchmark echo after upload receive",
                Some(connection_key.0),
                &connection,
            );
            b"throughput-ok".to_vec()
        } else if let Some(bytes) = parse_benchmark_size_request(&request, "throughput-download:")?
        {
            let stats_before = hub.stats_snapshot();
            let started_ms = benchmark_now_ms();
            write_benchmark_bytes(send, bytes).await?;
            let duration_ms = benchmark_now_ms() - started_ms;
            let stats = hub.stats_snapshot().delta_since(stats_before);
            log_throughput_phase_stats(
                connection_key,
                "echo-download-send",
                duration_ms,
                bytes,
                &stats,
            );
            trace_iroh_connection_paths(
                "benchmark echo after download send",
                Some(connection_key.0),
                &connection,
            );
            Vec::new()
        } else {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "benchmark echo received an unsupported request",
            ));
        };
        if !response.is_empty() {
            write_benchmark_frame(send, &response).await?;
        }
    }
    let _ = send.finish();
    Ok(())
}

async fn run_upload_benchmark_round(
    send: &mut SendStream,
    recv: &mut RecvStream,
    bytes: usize,
) -> BrowserWorkerResult<()> {
    write_benchmark_frame(send, format!("throughput-upload:{bytes}").as_bytes()).await?;
    write_benchmark_bytes(send, bytes).await?;
    let response = read_benchmark_frame(recv).await?;
    if response != b"throughput-ok" {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "throughput upload returned an invalid ack",
        ));
    }
    Ok(())
}

async fn run_download_benchmark_round(
    send: &mut SendStream,
    recv: &mut RecvStream,
    bytes: usize,
) -> BrowserWorkerResult<()> {
    write_benchmark_frame(send, format!("throughput-download:{bytes}").as_bytes()).await?;
    discard_benchmark_bytes(recv, bytes).await
}

async fn write_benchmark_bytes(send: &mut SendStream, mut len: usize) -> BrowserWorkerResult<()> {
    const ZERO_CHUNK: &[u8; 64 * 1024] = &[0; 64 * 1024];
    while len > 0 {
        let chunk_len = len.min(ZERO_CHUNK.len());
        send.write_all(&ZERO_CHUNK[..chunk_len])
            .await
            .map_err(|err| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    format!("failed to write benchmark bytes: {err}"),
                )
            })?;
        len -= chunk_len;
    }
    Ok(())
}

async fn discard_benchmark_bytes(recv: &mut RecvStream, mut len: usize) -> BrowserWorkerResult<()> {
    let mut buffer = vec![0; 64 * 1024];
    while len > 0 {
        let chunk_len = len.min(buffer.len());
        read_exact_benchmark(recv, &mut buffer[..chunk_len])
            .await?
            .then_some(())
            .ok_or_else(|| {
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    "benchmark stream closed before all bytes were received",
                )
            })?;
        len -= chunk_len;
    }
    Ok(())
}

async fn write_benchmark_frame(send: &mut SendStream, payload: &[u8]) -> BrowserWorkerResult<()> {
    if payload.len() > BENCHMARK_MAX_FRAME_BYTES {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "benchmark frame is too large",
        ));
    }
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    send.write_all(&frame).await.map_err(|err| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            format!("failed to write benchmark frame: {err}"),
        )
    })
}

async fn read_benchmark_frame(recv: &mut RecvStream) -> BrowserWorkerResult<Vec<u8>> {
    read_optional_benchmark_frame(recv).await?.ok_or_else(|| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "benchmark stream closed before response",
        )
    })
}

async fn read_optional_benchmark_frame(
    recv: &mut RecvStream,
) -> BrowserWorkerResult<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    if !read_exact_benchmark(recv, &mut len).await? {
        return Ok(None);
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > BENCHMARK_MAX_FRAME_BYTES {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "benchmark frame exceeds maximum size",
        ));
    }
    let mut payload = vec![0; len];
    if !read_exact_benchmark(recv, &mut payload).await? {
        return Err(BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "benchmark stream closed in the middle of a frame",
        ));
    }
    Ok(Some(payload))
}

async fn read_exact_benchmark(
    recv: &mut RecvStream,
    mut out: &mut [u8],
) -> BrowserWorkerResult<bool> {
    while !out.is_empty() {
        let len = recv.read(out).await.map_err(|err| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("failed to read benchmark frame: {err}"),
            )
        })?;
        let Some(len) = len else {
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

fn benchmark_percentile(sorted_samples: &[f64], percentile: f64) -> f64 {
    let index = ((sorted_samples.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted_samples.len() - 1);
    sorted_samples[index]
}

fn parse_benchmark_size_request(
    request: &[u8],
    prefix: &str,
) -> BrowserWorkerResult<Option<usize>> {
    let Some(rest) = request.strip_prefix(prefix.as_bytes()) else {
        return Ok(None);
    };
    let value = std::str::from_utf8(rest).map_err(|_| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "benchmark size request is not valid UTF-8",
        )
    })?;
    value.parse::<usize>().map(Some).map_err(|_| {
        BrowserWorkerError::new(
            BrowserWorkerErrorCode::WebRtcFailed,
            "benchmark size request is not a number",
        )
    })
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
fn benchmark_now_ms() -> f64 {
    performance_now_ms().unwrap_or_else(js_sys::Date::now)
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
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

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
fn benchmark_now_ms() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}
