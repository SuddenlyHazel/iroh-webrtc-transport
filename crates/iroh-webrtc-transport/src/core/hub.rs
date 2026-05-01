//! Packet routing and per-session queues for the WebRTC custom transport.

use std::{
    collections::{HashMap, VecDeque},
    future::poll_fn,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
};

use iroh_base::CustomAddr;
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

use crate::{
    config::{DEFAULT_LOCAL_ICE_QUEUE_CAPACITY, DEFAULT_SESSION_QUEUE_CAPACITY},
    core::{
        addr::{WebRtcAddr, WebRtcAddrKind},
        frame::WebRtcPacketFrame,
    },
    error::{Error, Result},
};

#[derive(Debug, Clone)]
pub struct QueuedPacket {
    pub source: CustomAddr,
    pub frame: WebRtcPacketFrame,
}

#[derive(Debug, Clone)]
pub struct OutboundPacket {
    pub destination: CustomAddr,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
/// Serializable WebRTC ICE candidate exchanged by the signaling layer.
pub struct WebRtcIceCandidate {
    /// Browser/native SDP candidate string.
    pub candidate: String,
    /// Optional SDP media id for the candidate.
    pub sdp_mid: Option<String>,
    /// Optional SDP media line index for the candidate.
    pub sdp_mline_index: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
/// Local ICE event emitted by a native or browser WebRTC peer connection.
pub enum LocalIceEvent {
    /// A gathered ICE candidate.
    Candidate(WebRtcIceCandidate),
    /// Candidate gathering completed.
    EndOfCandidates,
}

#[derive(Debug)]
struct LocalIceState {
    queue: VecDeque<LocalIceEvent>,
    capacity: usize,
    waker: Option<Waker>,
    closed: bool,
}

#[derive(Debug, Clone)]
/// Bounded queue of locally gathered ICE events.
pub struct LocalIceQueue {
    state: Arc<Mutex<LocalIceState>>,
}

#[derive(Debug)]
struct SessionState {
    recv: VecDeque<QueuedPacket>,
    outbound: HashMap<[u8; 16], OutboundRoute>,
    recv_waker: Option<Waker>,
    recv_capacity: usize,
    outbound_capacity: usize,
    closed: bool,
}

#[derive(Debug)]
struct OutboundRoute {
    sender: PollSender<OutboundPacket>,
    pending_waker: Option<Waker>,
}

pub type OutboundPacketReceiver = mpsc::Receiver<OutboundPacket>;

#[derive(Debug, Clone, Default)]
pub struct SessionHub {
    state: Arc<Mutex<SessionState>>,
    stats: Arc<WebRtcTransportStats>,
}

#[derive(Debug, Default)]
struct WebRtcTransportStats {
    sender_poll_send_calls: AtomicU64,
    sender_last_poll_send_ms: AtomicU64,
    sender_poll_send_gap_samples: AtomicU64,
    sender_poll_send_gap_ms_total: AtomicU64,
    sender_poll_send_gap_ms_max: AtomicU64,
    sender_poll_send_gap_over_1ms: AtomicU64,
    sender_poll_send_gap_over_4ms: AtomicU64,
    sender_poll_send_gap_over_16ms: AtomicU64,
    sender_transmits: AtomicU64,
    sender_payload_bytes: AtomicU64,
    sender_frame_bytes: AtomicU64,
    sender_transmit_datagrams: AtomicU64,
    sender_transmit_datagrams_max: AtomicU64,
    sender_transmits_single_datagram: AtomicU64,
    sender_transmits_segmented: AtomicU64,
    sender_transmits_datagrams_2_to_4: AtomicU64,
    sender_transmits_datagrams_5_to_9: AtomicU64,
    sender_transmits_full_batch: AtomicU64,
    sender_transmit_payload_max: AtomicU64,
    sender_last_transmit_ms: AtomicU64,
    sender_inter_transmit_gap_samples: AtomicU64,
    sender_inter_transmit_gap_ms_total: AtomicU64,
    sender_inter_transmit_gap_ms_max: AtomicU64,
    sender_inter_transmit_gap_over_1ms: AtomicU64,
    sender_inter_transmit_gap_over_4ms: AtomicU64,
    sender_inter_transmit_gap_over_16ms: AtomicU64,
    sender_encode_failures: AtomicU64,
    sender_queue_failures: AtomicU64,
    hub_last_recv_push_ms: AtomicU64,
    hub_recv_push_gap_samples: AtomicU64,
    hub_recv_push_gap_ms_total: AtomicU64,
    hub_recv_push_gap_ms_max: AtomicU64,
    hub_recv_push_gap_over_1ms: AtomicU64,
    hub_recv_push_gap_over_4ms: AtomicU64,
    hub_recv_push_gap_over_16ms: AtomicU64,
    hub_last_recv_wake_ms: AtomicU64,
    hub_recv_wake_to_poll_gap_samples: AtomicU64,
    hub_recv_wake_to_poll_gap_ms_total: AtomicU64,
    hub_recv_wake_to_poll_gap_ms_max: AtomicU64,
    hub_recv_wake_to_poll_gap_over_1ms: AtomicU64,
    hub_recv_wake_to_poll_gap_over_4ms: AtomicU64,
    hub_recv_wake_to_poll_gap_over_16ms: AtomicU64,
    hub_recv_pushes: AtomicU64,
    hub_recv_waker_sets: AtomicU64,
    hub_recv_wakes: AtomicU64,
    hub_last_outbound_push_ms: AtomicU64,
    hub_outbound_push_gap_samples: AtomicU64,
    hub_outbound_push_gap_ms_total: AtomicU64,
    hub_outbound_push_gap_ms_max: AtomicU64,
    hub_outbound_push_gap_over_1ms: AtomicU64,
    hub_outbound_push_gap_over_4ms: AtomicU64,
    hub_outbound_push_gap_over_16ms: AtomicU64,
    hub_outbound_pushes: AtomicU64,
    hub_outbound_bytes: AtomicU64,
    hub_outbound_depth_max: AtomicU64,
    hub_outbound_full: AtomicU64,
    hub_outbound_waker_sets: AtomicU64,
    hub_outbound_wakes: AtomicU64,
    hub_outbound_queue_delay_samples: AtomicU64,
    hub_outbound_queue_delay_ms_total: AtomicU64,
    hub_outbound_queue_delay_ms_max: AtomicU64,
    hub_outbound_queue_delay_over_1ms: AtomicU64,
    hub_outbound_queue_delay_over_4ms: AtomicU64,
    hub_outbound_queue_delay_over_16ms: AtomicU64,
    hub_pop_matching_calls: AtomicU64,
    hub_pop_matching_hits: AtomicU64,
    hub_pop_matching_pending: AtomicU64,
    hub_pop_matching_scan_total: AtomicU64,
    hub_pop_matching_scan_max: AtomicU64,
    data_channel_last_rx_message_ms: AtomicU64,
    data_channel_rx_message_gap_samples: AtomicU64,
    data_channel_rx_message_gap_ms_total: AtomicU64,
    data_channel_rx_message_gap_ms_max: AtomicU64,
    data_channel_rx_message_gap_over_1ms: AtomicU64,
    data_channel_rx_message_gap_over_4ms: AtomicU64,
    data_channel_rx_message_gap_over_16ms: AtomicU64,
    data_channel_rx_messages: AtomicU64,
    data_channel_rx_frame_bytes: AtomicU64,
    data_channel_rx_payload_bytes: AtomicU64,
    data_channel_rx_invalid_frames: AtomicU64,
    data_channel_rx_wrong_session: AtomicU64,
    data_channel_rx_enqueue_failures: AtomicU64,
    data_channel_last_pump_pop_ms: AtomicU64,
    data_channel_pump_pop_gap_samples: AtomicU64,
    data_channel_pump_pop_gap_ms_total: AtomicU64,
    data_channel_pump_pop_gap_ms_max: AtomicU64,
    data_channel_pump_pop_gap_over_1ms: AtomicU64,
    data_channel_pump_pop_gap_over_4ms: AtomicU64,
    data_channel_pump_pop_gap_over_16ms: AtomicU64,
    data_channel_pump_pops: AtomicU64,
    data_channel_last_send_ms: AtomicU64,
    data_channel_send_gap_samples: AtomicU64,
    data_channel_send_gap_ms_total: AtomicU64,
    data_channel_send_gap_ms_max: AtomicU64,
    data_channel_send_gap_over_1ms: AtomicU64,
    data_channel_send_gap_over_4ms: AtomicU64,
    data_channel_send_gap_over_16ms: AtomicU64,
    data_channel_sent_messages: AtomicU64,
    data_channel_sent_bytes: AtomicU64,
    data_channel_buffered_amount_samples: AtomicU64,
    data_channel_buffered_amount_before_total: AtomicU64,
    data_channel_buffered_amount_after_total: AtomicU64,
    data_channel_buffered_amount_before_max: AtomicU64,
    data_channel_buffered_amount_max: AtomicU64,
    data_channel_buffered_amount_before_over_256k: AtomicU64,
    data_channel_buffered_amount_before_over_512k: AtomicU64,
    data_channel_buffered_amount_before_over_1m: AtomicU64,
    data_channel_buffered_amount_after_over_256k: AtomicU64,
    data_channel_buffered_amount_after_over_512k: AtomicU64,
    data_channel_buffered_amount_after_over_1m: AtomicU64,
    data_channel_send_failures: AtomicU64,
    endpoint_last_poll_recv_call_ms: AtomicU64,
    endpoint_poll_recv_call_gap_samples: AtomicU64,
    endpoint_poll_recv_call_gap_ms_total: AtomicU64,
    endpoint_poll_recv_call_gap_ms_max: AtomicU64,
    endpoint_poll_recv_call_gap_over_1ms: AtomicU64,
    endpoint_poll_recv_call_gap_over_4ms: AtomicU64,
    endpoint_poll_recv_call_gap_over_16ms: AtomicU64,
    endpoint_poll_recv_calls: AtomicU64,
    endpoint_poll_recv_pending: AtomicU64,
    endpoint_poll_recv_ready: AtomicU64,
    endpoint_poll_recv_ready_packets: AtomicU64,
    endpoint_poll_recv_ready_packets_max: AtomicU64,
    endpoint_poll_recv_partial_pending: AtomicU64,
    endpoint_poll_recv_full: AtomicU64,
    endpoint_poll_recv_pending_frame_continuations: AtomicU64,
    endpoint_delivered_packets: AtomicU64,
    endpoint_delivered_payload_bytes: AtomicU64,
    endpoint_delivered_payload_max: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRtcTransportStatsSnapshot {
    pub sender_poll_send_calls: u64,
    pub sender_poll_send_gap_samples: u64,
    pub sender_poll_send_gap_ms_total: u64,
    pub sender_poll_send_gap_ms_max: u64,
    pub sender_poll_send_gap_over_1ms: u64,
    pub sender_poll_send_gap_over_4ms: u64,
    pub sender_poll_send_gap_over_16ms: u64,
    pub sender_transmits: u64,
    pub sender_payload_bytes: u64,
    pub sender_frame_bytes: u64,
    pub sender_transmit_datagrams: u64,
    pub sender_transmit_datagrams_max: u64,
    pub sender_transmits_single_datagram: u64,
    pub sender_transmits_segmented: u64,
    pub sender_transmits_datagrams_2_to_4: u64,
    pub sender_transmits_datagrams_5_to_9: u64,
    pub sender_transmits_full_batch: u64,
    pub sender_transmit_payload_max: u64,
    pub sender_inter_transmit_gap_samples: u64,
    pub sender_inter_transmit_gap_ms_total: u64,
    pub sender_inter_transmit_gap_ms_max: u64,
    pub sender_inter_transmit_gap_over_1ms: u64,
    pub sender_inter_transmit_gap_over_4ms: u64,
    pub sender_inter_transmit_gap_over_16ms: u64,
    pub sender_encode_failures: u64,
    pub sender_queue_failures: u64,
    pub hub_recv_push_gap_samples: u64,
    pub hub_recv_push_gap_ms_total: u64,
    pub hub_recv_push_gap_ms_max: u64,
    pub hub_recv_push_gap_over_1ms: u64,
    pub hub_recv_push_gap_over_4ms: u64,
    pub hub_recv_push_gap_over_16ms: u64,
    pub hub_recv_wake_to_poll_gap_samples: u64,
    pub hub_recv_wake_to_poll_gap_ms_total: u64,
    pub hub_recv_wake_to_poll_gap_ms_max: u64,
    pub hub_recv_wake_to_poll_gap_over_1ms: u64,
    pub hub_recv_wake_to_poll_gap_over_4ms: u64,
    pub hub_recv_wake_to_poll_gap_over_16ms: u64,
    pub hub_recv_pushes: u64,
    pub hub_recv_waker_sets: u64,
    pub hub_recv_wakes: u64,
    pub hub_outbound_push_gap_samples: u64,
    pub hub_outbound_push_gap_ms_total: u64,
    pub hub_outbound_push_gap_ms_max: u64,
    pub hub_outbound_push_gap_over_1ms: u64,
    pub hub_outbound_push_gap_over_4ms: u64,
    pub hub_outbound_push_gap_over_16ms: u64,
    pub hub_outbound_pushes: u64,
    pub hub_outbound_bytes: u64,
    pub hub_outbound_depth_max: u64,
    pub hub_outbound_full: u64,
    pub hub_outbound_waker_sets: u64,
    pub hub_outbound_wakes: u64,
    pub hub_outbound_queue_delay_samples: u64,
    pub hub_outbound_queue_delay_ms_total: u64,
    pub hub_outbound_queue_delay_ms_max: u64,
    pub hub_outbound_queue_delay_over_1ms: u64,
    pub hub_outbound_queue_delay_over_4ms: u64,
    pub hub_outbound_queue_delay_over_16ms: u64,
    pub hub_pop_matching_calls: u64,
    pub hub_pop_matching_hits: u64,
    pub hub_pop_matching_pending: u64,
    pub hub_pop_matching_scan_total: u64,
    pub hub_pop_matching_scan_max: u64,
    pub data_channel_rx_message_gap_samples: u64,
    pub data_channel_rx_message_gap_ms_total: u64,
    pub data_channel_rx_message_gap_ms_max: u64,
    pub data_channel_rx_message_gap_over_1ms: u64,
    pub data_channel_rx_message_gap_over_4ms: u64,
    pub data_channel_rx_message_gap_over_16ms: u64,
    pub data_channel_rx_messages: u64,
    pub data_channel_rx_frame_bytes: u64,
    pub data_channel_rx_payload_bytes: u64,
    pub data_channel_rx_invalid_frames: u64,
    pub data_channel_rx_wrong_session: u64,
    pub data_channel_rx_enqueue_failures: u64,
    pub data_channel_pump_pop_gap_samples: u64,
    pub data_channel_pump_pop_gap_ms_total: u64,
    pub data_channel_pump_pop_gap_ms_max: u64,
    pub data_channel_pump_pop_gap_over_1ms: u64,
    pub data_channel_pump_pop_gap_over_4ms: u64,
    pub data_channel_pump_pop_gap_over_16ms: u64,
    pub data_channel_pump_pops: u64,
    pub data_channel_send_gap_samples: u64,
    pub data_channel_send_gap_ms_total: u64,
    pub data_channel_send_gap_ms_max: u64,
    pub data_channel_send_gap_over_1ms: u64,
    pub data_channel_send_gap_over_4ms: u64,
    pub data_channel_send_gap_over_16ms: u64,
    pub data_channel_sent_messages: u64,
    pub data_channel_sent_bytes: u64,
    pub data_channel_buffered_amount_samples: u64,
    pub data_channel_buffered_amount_before_total: u64,
    pub data_channel_buffered_amount_after_total: u64,
    pub data_channel_buffered_amount_before_max: u64,
    pub data_channel_buffered_amount_max: u64,
    pub data_channel_buffered_amount_before_over_256k: u64,
    pub data_channel_buffered_amount_before_over_512k: u64,
    pub data_channel_buffered_amount_before_over_1m: u64,
    pub data_channel_buffered_amount_after_over_256k: u64,
    pub data_channel_buffered_amount_after_over_512k: u64,
    pub data_channel_buffered_amount_after_over_1m: u64,
    pub data_channel_send_failures: u64,
    pub endpoint_poll_recv_call_gap_samples: u64,
    pub endpoint_poll_recv_call_gap_ms_total: u64,
    pub endpoint_poll_recv_call_gap_ms_max: u64,
    pub endpoint_poll_recv_call_gap_over_1ms: u64,
    pub endpoint_poll_recv_call_gap_over_4ms: u64,
    pub endpoint_poll_recv_call_gap_over_16ms: u64,
    pub endpoint_poll_recv_calls: u64,
    pub endpoint_poll_recv_pending: u64,
    pub endpoint_poll_recv_ready: u64,
    pub endpoint_poll_recv_ready_packets: u64,
    pub endpoint_poll_recv_ready_packets_max: u64,
    pub endpoint_poll_recv_partial_pending: u64,
    pub endpoint_poll_recv_full: u64,
    pub endpoint_poll_recv_pending_frame_continuations: u64,
    pub endpoint_delivered_packets: u64,
    pub endpoint_delivered_payload_bytes: u64,
    pub endpoint_delivered_payload_max: u64,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            recv: VecDeque::new(),
            outbound: HashMap::new(),
            recv_waker: None,
            recv_capacity: DEFAULT_SESSION_QUEUE_CAPACITY,
            outbound_capacity: DEFAULT_SESSION_QUEUE_CAPACITY,
            closed: false,
        }
    }
}

impl Default for LocalIceState {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            capacity: DEFAULT_LOCAL_ICE_QUEUE_CAPACITY,
            waker: None,
            closed: false,
        }
    }
}

impl Default for LocalIceQueue {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(LocalIceState::default())),
        }
    }
}

impl LocalIceQueue {
    /// Create an ICE event queue with an explicit capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(LocalIceState {
                capacity,
                ..LocalIceState::default()
            })),
        }
    }

    /// Push one local ICE event into the queue.
    pub fn push(&self, event: LocalIceEvent) -> Result<()> {
        let mut state = self.state.lock().expect("local ICE mutex poisoned");
        if state.closed {
            return Err(Error::SessionClosed);
        }
        if state.queue.len() >= state.capacity {
            if event == LocalIceEvent::EndOfCandidates && state.capacity > 0 {
                if state
                    .queue
                    .iter()
                    .any(|queued| queued == &LocalIceEvent::EndOfCandidates)
                {
                    return Ok(());
                }
                state.queue.pop_front();
            } else {
                return Err(Error::SendQueueFull);
            }
        }
        state.queue.push_back(event);
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
        Ok(())
    }

    /// Poll for the next local ICE event.
    pub fn poll_next(&self, cx: &mut std::task::Context<'_>) -> Poll<Result<LocalIceEvent>> {
        let mut state = self.state.lock().expect("local ICE mutex poisoned");
        if let Some(event) = state.queue.pop_front() {
            return Poll::Ready(Ok(event));
        }
        if state.closed {
            return Poll::Ready(Err(Error::SessionClosed));
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }

    /// Wait for the next local ICE event.
    pub async fn next(&self) -> Result<LocalIceEvent> {
        poll_fn(|cx| self.poll_next(cx)).await
    }

    /// Close the ICE event queue and wake any waiter.
    pub fn close(&self) {
        let mut state = self.state.lock().expect("local ICE mutex poisoned");
        state.closed = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

impl SessionHub {
    pub fn with_capacities(recv_capacity: usize, outbound_capacity: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState {
                recv_capacity,
                outbound_capacity,
                ..SessionState::default()
            })),
            stats: Arc::new(WebRtcTransportStats::default()),
        }
    }

    pub fn stats_snapshot(&self) -> WebRtcTransportStatsSnapshot {
        self.stats.snapshot()
    }

    pub fn record_sender_poll_send_call(&self) {
        self.stats
            .sender_poll_send_calls
            .fetch_add(1, Ordering::Relaxed);
        record_gap_ms(
            &self.stats.sender_last_poll_send_ms,
            &self.stats.sender_poll_send_gap_samples,
            &self.stats.sender_poll_send_gap_ms_total,
            &self.stats.sender_poll_send_gap_ms_max,
            &self.stats.sender_poll_send_gap_over_1ms,
            &self.stats.sender_poll_send_gap_over_4ms,
            &self.stats.sender_poll_send_gap_over_16ms,
        );
    }

    pub fn record_sender_transmit(
        &self,
        payload_bytes: usize,
        frame_bytes: usize,
        datagrams: usize,
    ) {
        self.stats.sender_transmits.fetch_add(1, Ordering::Relaxed);
        self.stats
            .sender_payload_bytes
            .fetch_add(payload_bytes as u64, Ordering::Relaxed);
        self.stats
            .sender_frame_bytes
            .fetch_add(frame_bytes as u64, Ordering::Relaxed);
        self.stats
            .sender_transmit_datagrams
            .fetch_add(datagrams as u64, Ordering::Relaxed);
        update_max(&self.stats.sender_transmit_datagrams_max, datagrams as u64);
        update_max(
            &self.stats.sender_transmit_payload_max,
            payload_bytes as u64,
        );
        match datagrams {
            0 | 1 => {
                self.stats
                    .sender_transmits_single_datagram
                    .fetch_add(1, Ordering::Relaxed);
            }
            2..=4 => {
                self.stats
                    .sender_transmits_segmented
                    .fetch_add(1, Ordering::Relaxed);
                self.stats
                    .sender_transmits_datagrams_2_to_4
                    .fetch_add(1, Ordering::Relaxed);
            }
            5..=9 => {
                self.stats
                    .sender_transmits_segmented
                    .fetch_add(1, Ordering::Relaxed);
                self.stats
                    .sender_transmits_datagrams_5_to_9
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                self.stats
                    .sender_transmits_segmented
                    .fetch_add(1, Ordering::Relaxed);
                self.stats
                    .sender_transmits_full_batch
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        record_gap_ms(
            &self.stats.sender_last_transmit_ms,
            &self.stats.sender_inter_transmit_gap_samples,
            &self.stats.sender_inter_transmit_gap_ms_total,
            &self.stats.sender_inter_transmit_gap_ms_max,
            &self.stats.sender_inter_transmit_gap_over_1ms,
            &self.stats.sender_inter_transmit_gap_over_4ms,
            &self.stats.sender_inter_transmit_gap_over_16ms,
        );
    }

    pub fn record_sender_encode_failure(&self) {
        self.stats
            .sender_encode_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_sender_queue_failure(&self) {
        self.stats
            .sender_queue_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_data_channel_rx_message(&self, frame_bytes: usize, payload_bytes: usize) {
        self.stats
            .data_channel_rx_messages
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .data_channel_rx_frame_bytes
            .fetch_add(frame_bytes as u64, Ordering::Relaxed);
        self.stats
            .data_channel_rx_payload_bytes
            .fetch_add(payload_bytes as u64, Ordering::Relaxed);
        record_gap_ms(
            &self.stats.data_channel_last_rx_message_ms,
            &self.stats.data_channel_rx_message_gap_samples,
            &self.stats.data_channel_rx_message_gap_ms_total,
            &self.stats.data_channel_rx_message_gap_ms_max,
            &self.stats.data_channel_rx_message_gap_over_1ms,
            &self.stats.data_channel_rx_message_gap_over_4ms,
            &self.stats.data_channel_rx_message_gap_over_16ms,
        );
    }

    pub fn record_data_channel_rx_invalid_frame(&self) {
        self.stats
            .data_channel_rx_invalid_frames
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_data_channel_rx_wrong_session(&self) {
        self.stats
            .data_channel_rx_wrong_session
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_data_channel_rx_enqueue_failure(&self) {
        self.stats
            .data_channel_rx_enqueue_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_data_channel_pump_pop(&self) {
        self.stats
            .data_channel_pump_pops
            .fetch_add(1, Ordering::Relaxed);
        record_gap_ms(
            &self.stats.data_channel_last_pump_pop_ms,
            &self.stats.data_channel_pump_pop_gap_samples,
            &self.stats.data_channel_pump_pop_gap_ms_total,
            &self.stats.data_channel_pump_pop_gap_ms_max,
            &self.stats.data_channel_pump_pop_gap_over_1ms,
            &self.stats.data_channel_pump_pop_gap_over_4ms,
            &self.stats.data_channel_pump_pop_gap_over_16ms,
        );
    }

    pub fn record_data_channel_send(
        &self,
        bytes: usize,
        buffered_amount_before: u64,
        buffered_amount_after: u64,
    ) {
        self.stats
            .data_channel_sent_messages
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .data_channel_sent_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        self.stats
            .data_channel_buffered_amount_samples
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .data_channel_buffered_amount_before_total
            .fetch_add(buffered_amount_before, Ordering::Relaxed);
        self.stats
            .data_channel_buffered_amount_after_total
            .fetch_add(buffered_amount_after, Ordering::Relaxed);
        update_max(
            &self.stats.data_channel_buffered_amount_before_max,
            buffered_amount_before,
        );
        update_max(
            &self.stats.data_channel_buffered_amount_max,
            buffered_amount_after,
        );
        record_buffered_amount_thresholds(
            buffered_amount_before,
            &self.stats.data_channel_buffered_amount_before_over_256k,
            &self.stats.data_channel_buffered_amount_before_over_512k,
            &self.stats.data_channel_buffered_amount_before_over_1m,
        );
        record_buffered_amount_thresholds(
            buffered_amount_after,
            &self.stats.data_channel_buffered_amount_after_over_256k,
            &self.stats.data_channel_buffered_amount_after_over_512k,
            &self.stats.data_channel_buffered_amount_after_over_1m,
        );
        record_gap_ms(
            &self.stats.data_channel_last_send_ms,
            &self.stats.data_channel_send_gap_samples,
            &self.stats.data_channel_send_gap_ms_total,
            &self.stats.data_channel_send_gap_ms_max,
            &self.stats.data_channel_send_gap_over_1ms,
            &self.stats.data_channel_send_gap_over_4ms,
            &self.stats.data_channel_send_gap_over_16ms,
        );
    }

    pub fn record_data_channel_send_failure(&self) {
        self.stats
            .data_channel_send_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_endpoint_poll_recv_call(&self) {
        self.stats
            .endpoint_poll_recv_calls
            .fetch_add(1, Ordering::Relaxed);
        record_gap_ms(
            &self.stats.endpoint_last_poll_recv_call_ms,
            &self.stats.endpoint_poll_recv_call_gap_samples,
            &self.stats.endpoint_poll_recv_call_gap_ms_total,
            &self.stats.endpoint_poll_recv_call_gap_ms_max,
            &self.stats.endpoint_poll_recv_call_gap_over_1ms,
            &self.stats.endpoint_poll_recv_call_gap_over_4ms,
            &self.stats.endpoint_poll_recv_call_gap_over_16ms,
        );
        let last_wake = self.stats.hub_last_recv_wake_ms.swap(0, Ordering::Relaxed);
        if last_wake != 0 {
            record_observed_gap_ms(
                transport_now_ms().saturating_sub(last_wake),
                &self.stats.hub_recv_wake_to_poll_gap_samples,
                &self.stats.hub_recv_wake_to_poll_gap_ms_total,
                &self.stats.hub_recv_wake_to_poll_gap_ms_max,
                &self.stats.hub_recv_wake_to_poll_gap_over_1ms,
                &self.stats.hub_recv_wake_to_poll_gap_over_4ms,
                &self.stats.hub_recv_wake_to_poll_gap_over_16ms,
            );
        }
    }

    pub fn record_endpoint_poll_recv_pending(&self) {
        self.stats
            .endpoint_poll_recv_pending
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_endpoint_poll_recv_ready(&self, packets: usize, full: bool) {
        self.stats
            .endpoint_poll_recv_ready
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .endpoint_poll_recv_ready_packets
            .fetch_add(packets as u64, Ordering::Relaxed);
        update_max(
            &self.stats.endpoint_poll_recv_ready_packets_max,
            packets as u64,
        );
        if full {
            self.stats
                .endpoint_poll_recv_full
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn record_endpoint_poll_recv_partial_pending(&self) {
        self.stats
            .endpoint_poll_recv_partial_pending
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_endpoint_poll_recv_pending_frame_continuation(&self) {
        self.stats
            .endpoint_poll_recv_pending_frame_continuations
            .fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_endpoint_delivery(
        &self,
        payload_bytes: usize,
        datagrams: usize,
        max_datagram_bytes: usize,
    ) {
        self.stats
            .endpoint_delivered_packets
            .fetch_add(datagrams as u64, Ordering::Relaxed);
        self.stats
            .endpoint_delivered_payload_bytes
            .fetch_add(payload_bytes as u64, Ordering::Relaxed);
        update_max(
            &self.stats.endpoint_delivered_payload_max,
            max_datagram_bytes as u64,
        );
    }

    pub fn push_received(&self, packet: QueuedPacket) -> Result<()> {
        let mut state = self.state.lock().expect("session mutex poisoned");
        if state.closed {
            tracing::debug!(
                target: "iroh_webrtc_transport::hub",
                source = ?packet.source,
                session_id = ?packet.frame.session_id,
                "dropping received WebRTC packet because session hub is closed"
            );
            return Err(Error::SessionClosed);
        }
        if state.recv.len() >= state.recv_capacity {
            tracing::debug!(
                target: "iroh_webrtc_transport::hub",
                source = ?packet.source,
                session_id = ?packet.frame.session_id,
                recv_len = state.recv.len(),
                recv_capacity = state.recv_capacity,
                "dropping received WebRTC packet because receive queue is full"
            );
            return Err(Error::SendQueueFull);
        }
        let recv_len = state.recv.len() + 1;
        let outbound_len = state.outbound.len();
        let had_waker = state.recv_waker.is_some();
        state.recv.push_back(packet);
        self.stats.hub_recv_pushes.fetch_add(1, Ordering::Relaxed);
        record_gap_ms(
            &self.stats.hub_last_recv_push_ms,
            &self.stats.hub_recv_push_gap_samples,
            &self.stats.hub_recv_push_gap_ms_total,
            &self.stats.hub_recv_push_gap_ms_max,
            &self.stats.hub_recv_push_gap_over_1ms,
            &self.stats.hub_recv_push_gap_over_4ms,
            &self.stats.hub_recv_push_gap_over_16ms,
        );
        if let Some(waker) = state.recv_waker.take() {
            self.stats.hub_recv_wakes.fetch_add(1, Ordering::Relaxed);
            self.stats
                .hub_last_recv_wake_ms
                .store(transport_now_ms(), Ordering::Relaxed);
            waker.wake();
        }
        tracing::trace!(
            target: "iroh_webrtc_transport::hub",
            recv_len,
            outbound_len,
            had_waker,
            "queued received WebRTC packet"
        );
        Ok(())
    }

    pub fn register_outbound_session(&self, session_id: [u8; 16]) -> OutboundPacketReceiver {
        let (sender, receiver) = mpsc::channel(self.outbound_capacity());
        let mut state = self.state.lock().expect("session mutex poisoned");
        if !state.closed {
            state.outbound.insert(
                session_id,
                OutboundRoute {
                    sender: PollSender::new(sender),
                    pending_waker: None,
                },
            );
        }
        receiver
    }

    fn outbound_capacity(&self) -> usize {
        let state = self.state.lock().expect("session mutex poisoned");
        state.outbound_capacity
    }

    pub fn poll_push_outbound(
        &self,
        cx: &mut Context<'_>,
        packet: OutboundPacket,
    ) -> Poll<Result<()>> {
        let session_id = match WebRtcPacketFrame::session_id(&packet.bytes) {
            Ok(session_id) => session_id,
            Err(err) => return Poll::Ready(Err(err)),
        };
        let mut state = self.state.lock().expect("session mutex poisoned");
        if state.closed {
            tracing::debug!(
                target: "iroh_webrtc_transport::hub",
                destination = ?packet.destination,
                packet_bytes = packet.bytes.len(),
                "dropping outbound WebRTC packet because session hub is closed"
            );
            return Poll::Ready(Err(Error::SessionClosed));
        }
        let recv_len = state.recv.len();
        let Some(route) = state.outbound.get_mut(&session_id) else {
            tracing::debug!(
                target: "iroh_webrtc_transport::hub",
                destination = ?packet.destination,
                session_id = ?session_id,
                packet_bytes = packet.bytes.len(),
                "dropping outbound WebRTC packet because outbound session is not registered"
            );
            return Poll::Ready(Err(Error::SessionClosed));
        };

        match route.sender.poll_reserve(cx) {
            Poll::Pending => {
                self.stats.hub_outbound_full.fetch_add(1, Ordering::Relaxed);
                let should_store = route
                    .pending_waker
                    .as_ref()
                    .is_none_or(|waker| !waker.will_wake(cx.waker()));
                if should_store {
                    route.pending_waker = Some(cx.waker().clone());
                    self.stats
                        .hub_outbound_waker_sets
                        .fetch_add(1, Ordering::Relaxed);
                }
                return Poll::Pending;
            }
            Poll::Ready(Err(_)) => {
                route.pending_waker = None;
                tracing::debug!(
                    target: "iroh_webrtc_transport::hub",
                    destination = ?packet.destination,
                    session_id = ?session_id,
                    packet_bytes = packet.bytes.len(),
                    "dropping outbound WebRTC packet because outbound session is closed"
                );
                return Poll::Ready(Err(Error::SessionClosed));
            }
            Poll::Ready(Ok(())) => {
                route.pending_waker = None;
            }
        }

        let packet_bytes = packet.bytes.len();
        let outbound_depth = route
            .sender
            .get_ref()
            .map(|sender| sender.max_capacity().saturating_sub(sender.capacity()))
            .unwrap_or(0);
        if let Err(err) = route.sender.send_item(packet) {
            tracing::debug!(
                target: "iroh_webrtc_transport::hub",
                session_id = ?session_id,
                error = ?err,
                "dropping outbound WebRTC packet because outbound session closed during send"
            );
            return Poll::Ready(Err(Error::SessionClosed));
        }
        self.stats
            .hub_outbound_pushes
            .fetch_add(1, Ordering::Relaxed);
        self.stats
            .hub_outbound_bytes
            .fetch_add(packet_bytes as u64, Ordering::Relaxed);
        update_max(
            &self.stats.hub_outbound_depth_max,
            outbound_depth.saturating_add(1) as u64,
        );
        record_gap_ms(
            &self.stats.hub_last_outbound_push_ms,
            &self.stats.hub_outbound_push_gap_samples,
            &self.stats.hub_outbound_push_gap_ms_total,
            &self.stats.hub_outbound_push_gap_ms_max,
            &self.stats.hub_outbound_push_gap_over_1ms,
            &self.stats.hub_outbound_push_gap_over_4ms,
            &self.stats.hub_outbound_push_gap_over_16ms,
        );
        tracing::trace!(
            target: "iroh_webrtc_transport::hub",
            recv_len,
            outbound_depth = outbound_depth + 1,
            session_id = ?session_id,
            packet_bytes,
            "queued outbound WebRTC packet in session channel"
        );
        Poll::Ready(Ok(()))
    }

    pub fn pop_received(
        &self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<QueuedPacket>> {
        let mut state = self.state.lock().expect("session mutex poisoned");
        if let Some(packet) = state.recv.pop_front() {
            tracing::trace!(
                target: "iroh_webrtc_transport::hub",
                source = ?packet.source,
                session_id = ?packet.frame.session_id,
                recv_len = state.recv.len(),
                "popped received WebRTC packet from session hub"
            );
            return Poll::Ready(Ok(packet));
        }
        if state.closed {
            return Poll::Ready(Err(io::Error::from(Error::SessionClosed)));
        }
        state.recv_waker = Some(cx.waker().clone());
        self.stats
            .hub_recv_waker_sets
            .fetch_add(1, Ordering::Relaxed);
        Poll::Pending
    }

    pub fn unregister_outbound_for(&self, destination: &CustomAddr) -> bool {
        let Some(session_id) = session_id_from_destination(destination) else {
            return false;
        };
        self.unregister_outbound_session(session_id)
    }

    pub fn unregister_outbound_session(&self, session_id: [u8; 16]) -> bool {
        let mut state = self.state.lock().expect("session mutex poisoned");
        let Some(mut route) = state.outbound.remove(&session_id) else {
            return false;
        };
        if let Some(waker) = route.pending_waker.take() {
            self.stats
                .hub_outbound_wakes
                .fetch_add(1, Ordering::Relaxed);
            waker.wake();
        }
        true
    }

    pub fn queue_depths(&self) -> (usize, usize) {
        let state = self.state.lock().expect("session mutex poisoned");
        let outbound_depth = state
            .outbound
            .values()
            .filter_map(|route| {
                route
                    .sender
                    .get_ref()
                    .map(|sender| sender.max_capacity().saturating_sub(sender.capacity()))
            })
            .sum();
        (state.recv.len(), outbound_depth)
    }

    pub fn close(&self) {
        let mut state = self.state.lock().expect("session mutex poisoned");
        state.closed = true;
        if let Some(waker) = state.recv_waker.take() {
            waker.wake();
        }
        for (_, mut route) in state.outbound.drain() {
            if let Some(waker) = route.pending_waker.take() {
                self.stats
                    .hub_outbound_wakes
                    .fetch_add(1, Ordering::Relaxed);
                waker.wake();
            }
        }
    }
}

fn session_id_from_destination(destination: &CustomAddr) -> Option<[u8; 16]> {
    match WebRtcAddr::from_custom_addr(destination).ok()?.kind {
        WebRtcAddrKind::Session { session_id } => Some(session_id),
        WebRtcAddrKind::Capability => None,
    }
}

fn record_buffered_amount_thresholds(
    amount: u64,
    over_256k: &AtomicU64,
    over_512k: &AtomicU64,
    over_1m: &AtomicU64,
) {
    if amount > 256 * 1024 {
        over_256k.fetch_add(1, Ordering::Relaxed);
    }
    if amount > 512 * 1024 {
        over_512k.fetch_add(1, Ordering::Relaxed);
    }
    if amount > 1024 * 1024 {
        over_1m.fetch_add(1, Ordering::Relaxed);
    }
}

fn update_max(value: &AtomicU64, candidate: u64) {
    let mut current = value.load(Ordering::Relaxed);
    while candidate > current {
        match value.compare_exchange_weak(current, candidate, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(next) => current = next,
        }
    }
}

fn record_gap_ms(
    last_ms: &AtomicU64,
    samples: &AtomicU64,
    total: &AtomicU64,
    max: &AtomicU64,
    over_1ms: &AtomicU64,
    over_4ms: &AtomicU64,
    over_16ms: &AtomicU64,
) {
    let now = transport_now_ms();
    let previous = last_ms.swap(now, Ordering::Relaxed);
    if previous != 0 {
        record_observed_gap_ms(
            now.saturating_sub(previous),
            samples,
            total,
            max,
            over_1ms,
            over_4ms,
            over_16ms,
        );
    }
}

fn record_observed_gap_ms(
    gap: u64,
    samples: &AtomicU64,
    total: &AtomicU64,
    max: &AtomicU64,
    over_1ms: &AtomicU64,
    over_4ms: &AtomicU64,
    over_16ms: &AtomicU64,
) {
    samples.fetch_add(1, Ordering::Relaxed);
    total.fetch_add(gap, Ordering::Relaxed);
    update_max(max, gap);
    if gap > 1 {
        over_1ms.fetch_add(1, Ordering::Relaxed);
    }
    if gap > 4 {
        over_4ms.fetch_add(1, Ordering::Relaxed);
    }
    if gap > 16 {
        over_16ms.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(all(target_family = "wasm", target_os = "unknown"))]
fn transport_now_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
fn transport_now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

impl WebRtcTransportStats {
    fn snapshot(&self) -> WebRtcTransportStatsSnapshot {
        WebRtcTransportStatsSnapshot {
            sender_poll_send_calls: self.sender_poll_send_calls.load(Ordering::Relaxed),
            sender_poll_send_gap_samples: self.sender_poll_send_gap_samples.load(Ordering::Relaxed),
            sender_poll_send_gap_ms_total: self
                .sender_poll_send_gap_ms_total
                .load(Ordering::Relaxed),
            sender_poll_send_gap_ms_max: self.sender_poll_send_gap_ms_max.load(Ordering::Relaxed),
            sender_poll_send_gap_over_1ms: self
                .sender_poll_send_gap_over_1ms
                .load(Ordering::Relaxed),
            sender_poll_send_gap_over_4ms: self
                .sender_poll_send_gap_over_4ms
                .load(Ordering::Relaxed),
            sender_poll_send_gap_over_16ms: self
                .sender_poll_send_gap_over_16ms
                .load(Ordering::Relaxed),
            sender_transmits: self.sender_transmits.load(Ordering::Relaxed),
            sender_payload_bytes: self.sender_payload_bytes.load(Ordering::Relaxed),
            sender_frame_bytes: self.sender_frame_bytes.load(Ordering::Relaxed),
            sender_transmit_datagrams: self.sender_transmit_datagrams.load(Ordering::Relaxed),
            sender_transmit_datagrams_max: self
                .sender_transmit_datagrams_max
                .load(Ordering::Relaxed),
            sender_transmits_single_datagram: self
                .sender_transmits_single_datagram
                .load(Ordering::Relaxed),
            sender_transmits_segmented: self.sender_transmits_segmented.load(Ordering::Relaxed),
            sender_transmits_datagrams_2_to_4: self
                .sender_transmits_datagrams_2_to_4
                .load(Ordering::Relaxed),
            sender_transmits_datagrams_5_to_9: self
                .sender_transmits_datagrams_5_to_9
                .load(Ordering::Relaxed),
            sender_transmits_full_batch: self.sender_transmits_full_batch.load(Ordering::Relaxed),
            sender_transmit_payload_max: self.sender_transmit_payload_max.load(Ordering::Relaxed),
            sender_inter_transmit_gap_samples: self
                .sender_inter_transmit_gap_samples
                .load(Ordering::Relaxed),
            sender_inter_transmit_gap_ms_total: self
                .sender_inter_transmit_gap_ms_total
                .load(Ordering::Relaxed),
            sender_inter_transmit_gap_ms_max: self
                .sender_inter_transmit_gap_ms_max
                .load(Ordering::Relaxed),
            sender_inter_transmit_gap_over_1ms: self
                .sender_inter_transmit_gap_over_1ms
                .load(Ordering::Relaxed),
            sender_inter_transmit_gap_over_4ms: self
                .sender_inter_transmit_gap_over_4ms
                .load(Ordering::Relaxed),
            sender_inter_transmit_gap_over_16ms: self
                .sender_inter_transmit_gap_over_16ms
                .load(Ordering::Relaxed),
            sender_encode_failures: self.sender_encode_failures.load(Ordering::Relaxed),
            sender_queue_failures: self.sender_queue_failures.load(Ordering::Relaxed),
            hub_recv_push_gap_samples: self.hub_recv_push_gap_samples.load(Ordering::Relaxed),
            hub_recv_push_gap_ms_total: self.hub_recv_push_gap_ms_total.load(Ordering::Relaxed),
            hub_recv_push_gap_ms_max: self.hub_recv_push_gap_ms_max.load(Ordering::Relaxed),
            hub_recv_push_gap_over_1ms: self.hub_recv_push_gap_over_1ms.load(Ordering::Relaxed),
            hub_recv_push_gap_over_4ms: self.hub_recv_push_gap_over_4ms.load(Ordering::Relaxed),
            hub_recv_push_gap_over_16ms: self.hub_recv_push_gap_over_16ms.load(Ordering::Relaxed),
            hub_recv_wake_to_poll_gap_samples: self
                .hub_recv_wake_to_poll_gap_samples
                .load(Ordering::Relaxed),
            hub_recv_wake_to_poll_gap_ms_total: self
                .hub_recv_wake_to_poll_gap_ms_total
                .load(Ordering::Relaxed),
            hub_recv_wake_to_poll_gap_ms_max: self
                .hub_recv_wake_to_poll_gap_ms_max
                .load(Ordering::Relaxed),
            hub_recv_wake_to_poll_gap_over_1ms: self
                .hub_recv_wake_to_poll_gap_over_1ms
                .load(Ordering::Relaxed),
            hub_recv_wake_to_poll_gap_over_4ms: self
                .hub_recv_wake_to_poll_gap_over_4ms
                .load(Ordering::Relaxed),
            hub_recv_wake_to_poll_gap_over_16ms: self
                .hub_recv_wake_to_poll_gap_over_16ms
                .load(Ordering::Relaxed),
            hub_recv_pushes: self.hub_recv_pushes.load(Ordering::Relaxed),
            hub_recv_waker_sets: self.hub_recv_waker_sets.load(Ordering::Relaxed),
            hub_recv_wakes: self.hub_recv_wakes.load(Ordering::Relaxed),
            hub_outbound_push_gap_samples: self
                .hub_outbound_push_gap_samples
                .load(Ordering::Relaxed),
            hub_outbound_push_gap_ms_total: self
                .hub_outbound_push_gap_ms_total
                .load(Ordering::Relaxed),
            hub_outbound_push_gap_ms_max: self.hub_outbound_push_gap_ms_max.load(Ordering::Relaxed),
            hub_outbound_push_gap_over_1ms: self
                .hub_outbound_push_gap_over_1ms
                .load(Ordering::Relaxed),
            hub_outbound_push_gap_over_4ms: self
                .hub_outbound_push_gap_over_4ms
                .load(Ordering::Relaxed),
            hub_outbound_push_gap_over_16ms: self
                .hub_outbound_push_gap_over_16ms
                .load(Ordering::Relaxed),
            hub_outbound_pushes: self.hub_outbound_pushes.load(Ordering::Relaxed),
            hub_outbound_bytes: self.hub_outbound_bytes.load(Ordering::Relaxed),
            hub_outbound_depth_max: self.hub_outbound_depth_max.load(Ordering::Relaxed),
            hub_outbound_full: self.hub_outbound_full.load(Ordering::Relaxed),
            hub_outbound_waker_sets: self.hub_outbound_waker_sets.load(Ordering::Relaxed),
            hub_outbound_wakes: self.hub_outbound_wakes.load(Ordering::Relaxed),
            hub_outbound_queue_delay_samples: self
                .hub_outbound_queue_delay_samples
                .load(Ordering::Relaxed),
            hub_outbound_queue_delay_ms_total: self
                .hub_outbound_queue_delay_ms_total
                .load(Ordering::Relaxed),
            hub_outbound_queue_delay_ms_max: self
                .hub_outbound_queue_delay_ms_max
                .load(Ordering::Relaxed),
            hub_outbound_queue_delay_over_1ms: self
                .hub_outbound_queue_delay_over_1ms
                .load(Ordering::Relaxed),
            hub_outbound_queue_delay_over_4ms: self
                .hub_outbound_queue_delay_over_4ms
                .load(Ordering::Relaxed),
            hub_outbound_queue_delay_over_16ms: self
                .hub_outbound_queue_delay_over_16ms
                .load(Ordering::Relaxed),
            hub_pop_matching_calls: self.hub_pop_matching_calls.load(Ordering::Relaxed),
            hub_pop_matching_hits: self.hub_pop_matching_hits.load(Ordering::Relaxed),
            hub_pop_matching_pending: self.hub_pop_matching_pending.load(Ordering::Relaxed),
            hub_pop_matching_scan_total: self.hub_pop_matching_scan_total.load(Ordering::Relaxed),
            hub_pop_matching_scan_max: self.hub_pop_matching_scan_max.load(Ordering::Relaxed),
            data_channel_rx_message_gap_samples: self
                .data_channel_rx_message_gap_samples
                .load(Ordering::Relaxed),
            data_channel_rx_message_gap_ms_total: self
                .data_channel_rx_message_gap_ms_total
                .load(Ordering::Relaxed),
            data_channel_rx_message_gap_ms_max: self
                .data_channel_rx_message_gap_ms_max
                .load(Ordering::Relaxed),
            data_channel_rx_message_gap_over_1ms: self
                .data_channel_rx_message_gap_over_1ms
                .load(Ordering::Relaxed),
            data_channel_rx_message_gap_over_4ms: self
                .data_channel_rx_message_gap_over_4ms
                .load(Ordering::Relaxed),
            data_channel_rx_message_gap_over_16ms: self
                .data_channel_rx_message_gap_over_16ms
                .load(Ordering::Relaxed),
            data_channel_rx_messages: self.data_channel_rx_messages.load(Ordering::Relaxed),
            data_channel_rx_frame_bytes: self.data_channel_rx_frame_bytes.load(Ordering::Relaxed),
            data_channel_rx_payload_bytes: self
                .data_channel_rx_payload_bytes
                .load(Ordering::Relaxed),
            data_channel_rx_invalid_frames: self
                .data_channel_rx_invalid_frames
                .load(Ordering::Relaxed),
            data_channel_rx_wrong_session: self
                .data_channel_rx_wrong_session
                .load(Ordering::Relaxed),
            data_channel_rx_enqueue_failures: self
                .data_channel_rx_enqueue_failures
                .load(Ordering::Relaxed),
            data_channel_pump_pop_gap_samples: self
                .data_channel_pump_pop_gap_samples
                .load(Ordering::Relaxed),
            data_channel_pump_pop_gap_ms_total: self
                .data_channel_pump_pop_gap_ms_total
                .load(Ordering::Relaxed),
            data_channel_pump_pop_gap_ms_max: self
                .data_channel_pump_pop_gap_ms_max
                .load(Ordering::Relaxed),
            data_channel_pump_pop_gap_over_1ms: self
                .data_channel_pump_pop_gap_over_1ms
                .load(Ordering::Relaxed),
            data_channel_pump_pop_gap_over_4ms: self
                .data_channel_pump_pop_gap_over_4ms
                .load(Ordering::Relaxed),
            data_channel_pump_pop_gap_over_16ms: self
                .data_channel_pump_pop_gap_over_16ms
                .load(Ordering::Relaxed),
            data_channel_pump_pops: self.data_channel_pump_pops.load(Ordering::Relaxed),
            data_channel_send_gap_samples: self
                .data_channel_send_gap_samples
                .load(Ordering::Relaxed),
            data_channel_send_gap_ms_total: self
                .data_channel_send_gap_ms_total
                .load(Ordering::Relaxed),
            data_channel_send_gap_ms_max: self.data_channel_send_gap_ms_max.load(Ordering::Relaxed),
            data_channel_send_gap_over_1ms: self
                .data_channel_send_gap_over_1ms
                .load(Ordering::Relaxed),
            data_channel_send_gap_over_4ms: self
                .data_channel_send_gap_over_4ms
                .load(Ordering::Relaxed),
            data_channel_send_gap_over_16ms: self
                .data_channel_send_gap_over_16ms
                .load(Ordering::Relaxed),
            data_channel_sent_messages: self.data_channel_sent_messages.load(Ordering::Relaxed),
            data_channel_sent_bytes: self.data_channel_sent_bytes.load(Ordering::Relaxed),
            data_channel_buffered_amount_samples: self
                .data_channel_buffered_amount_samples
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_before_total: self
                .data_channel_buffered_amount_before_total
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_after_total: self
                .data_channel_buffered_amount_after_total
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_before_max: self
                .data_channel_buffered_amount_before_max
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_max: self
                .data_channel_buffered_amount_max
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_before_over_256k: self
                .data_channel_buffered_amount_before_over_256k
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_before_over_512k: self
                .data_channel_buffered_amount_before_over_512k
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_before_over_1m: self
                .data_channel_buffered_amount_before_over_1m
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_after_over_256k: self
                .data_channel_buffered_amount_after_over_256k
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_after_over_512k: self
                .data_channel_buffered_amount_after_over_512k
                .load(Ordering::Relaxed),
            data_channel_buffered_amount_after_over_1m: self
                .data_channel_buffered_amount_after_over_1m
                .load(Ordering::Relaxed),
            data_channel_send_failures: self.data_channel_send_failures.load(Ordering::Relaxed),
            endpoint_poll_recv_call_gap_samples: self
                .endpoint_poll_recv_call_gap_samples
                .load(Ordering::Relaxed),
            endpoint_poll_recv_call_gap_ms_total: self
                .endpoint_poll_recv_call_gap_ms_total
                .load(Ordering::Relaxed),
            endpoint_poll_recv_call_gap_ms_max: self
                .endpoint_poll_recv_call_gap_ms_max
                .load(Ordering::Relaxed),
            endpoint_poll_recv_call_gap_over_1ms: self
                .endpoint_poll_recv_call_gap_over_1ms
                .load(Ordering::Relaxed),
            endpoint_poll_recv_call_gap_over_4ms: self
                .endpoint_poll_recv_call_gap_over_4ms
                .load(Ordering::Relaxed),
            endpoint_poll_recv_call_gap_over_16ms: self
                .endpoint_poll_recv_call_gap_over_16ms
                .load(Ordering::Relaxed),
            endpoint_poll_recv_calls: self.endpoint_poll_recv_calls.load(Ordering::Relaxed),
            endpoint_poll_recv_pending: self.endpoint_poll_recv_pending.load(Ordering::Relaxed),
            endpoint_poll_recv_ready: self.endpoint_poll_recv_ready.load(Ordering::Relaxed),
            endpoint_poll_recv_ready_packets: self
                .endpoint_poll_recv_ready_packets
                .load(Ordering::Relaxed),
            endpoint_poll_recv_ready_packets_max: self
                .endpoint_poll_recv_ready_packets_max
                .load(Ordering::Relaxed),
            endpoint_poll_recv_partial_pending: self
                .endpoint_poll_recv_partial_pending
                .load(Ordering::Relaxed),
            endpoint_poll_recv_full: self.endpoint_poll_recv_full.load(Ordering::Relaxed),
            endpoint_poll_recv_pending_frame_continuations: self
                .endpoint_poll_recv_pending_frame_continuations
                .load(Ordering::Relaxed),
            endpoint_delivered_packets: self.endpoint_delivered_packets.load(Ordering::Relaxed),
            endpoint_delivered_payload_bytes: self
                .endpoint_delivered_payload_bytes
                .load(Ordering::Relaxed),
            endpoint_delivered_payload_max: self
                .endpoint_delivered_payload_max
                .load(Ordering::Relaxed),
        }
    }
}

impl WebRtcTransportStatsSnapshot {
    pub fn delta_since(self, before: Self) -> Self {
        Self {
            sender_poll_send_calls: self
                .sender_poll_send_calls
                .saturating_sub(before.sender_poll_send_calls),
            sender_poll_send_gap_samples: self
                .sender_poll_send_gap_samples
                .saturating_sub(before.sender_poll_send_gap_samples),
            sender_poll_send_gap_ms_total: self
                .sender_poll_send_gap_ms_total
                .saturating_sub(before.sender_poll_send_gap_ms_total),
            sender_poll_send_gap_ms_max: self.sender_poll_send_gap_ms_max,
            sender_poll_send_gap_over_1ms: self
                .sender_poll_send_gap_over_1ms
                .saturating_sub(before.sender_poll_send_gap_over_1ms),
            sender_poll_send_gap_over_4ms: self
                .sender_poll_send_gap_over_4ms
                .saturating_sub(before.sender_poll_send_gap_over_4ms),
            sender_poll_send_gap_over_16ms: self
                .sender_poll_send_gap_over_16ms
                .saturating_sub(before.sender_poll_send_gap_over_16ms),
            sender_transmits: self
                .sender_transmits
                .saturating_sub(before.sender_transmits),
            sender_payload_bytes: self
                .sender_payload_bytes
                .saturating_sub(before.sender_payload_bytes),
            sender_frame_bytes: self
                .sender_frame_bytes
                .saturating_sub(before.sender_frame_bytes),
            sender_transmit_datagrams: self
                .sender_transmit_datagrams
                .saturating_sub(before.sender_transmit_datagrams),
            sender_transmit_datagrams_max: self.sender_transmit_datagrams_max,
            sender_transmits_single_datagram: self
                .sender_transmits_single_datagram
                .saturating_sub(before.sender_transmits_single_datagram),
            sender_transmits_segmented: self
                .sender_transmits_segmented
                .saturating_sub(before.sender_transmits_segmented),
            sender_transmits_datagrams_2_to_4: self
                .sender_transmits_datagrams_2_to_4
                .saturating_sub(before.sender_transmits_datagrams_2_to_4),
            sender_transmits_datagrams_5_to_9: self
                .sender_transmits_datagrams_5_to_9
                .saturating_sub(before.sender_transmits_datagrams_5_to_9),
            sender_transmits_full_batch: self
                .sender_transmits_full_batch
                .saturating_sub(before.sender_transmits_full_batch),
            sender_transmit_payload_max: self.sender_transmit_payload_max,
            sender_inter_transmit_gap_samples: self
                .sender_inter_transmit_gap_samples
                .saturating_sub(before.sender_inter_transmit_gap_samples),
            sender_inter_transmit_gap_ms_total: self
                .sender_inter_transmit_gap_ms_total
                .saturating_sub(before.sender_inter_transmit_gap_ms_total),
            sender_inter_transmit_gap_ms_max: self.sender_inter_transmit_gap_ms_max,
            sender_inter_transmit_gap_over_1ms: self
                .sender_inter_transmit_gap_over_1ms
                .saturating_sub(before.sender_inter_transmit_gap_over_1ms),
            sender_inter_transmit_gap_over_4ms: self
                .sender_inter_transmit_gap_over_4ms
                .saturating_sub(before.sender_inter_transmit_gap_over_4ms),
            sender_inter_transmit_gap_over_16ms: self
                .sender_inter_transmit_gap_over_16ms
                .saturating_sub(before.sender_inter_transmit_gap_over_16ms),
            sender_encode_failures: self
                .sender_encode_failures
                .saturating_sub(before.sender_encode_failures),
            sender_queue_failures: self
                .sender_queue_failures
                .saturating_sub(before.sender_queue_failures),
            hub_recv_push_gap_samples: self
                .hub_recv_push_gap_samples
                .saturating_sub(before.hub_recv_push_gap_samples),
            hub_recv_push_gap_ms_total: self
                .hub_recv_push_gap_ms_total
                .saturating_sub(before.hub_recv_push_gap_ms_total),
            hub_recv_push_gap_ms_max: self.hub_recv_push_gap_ms_max,
            hub_recv_push_gap_over_1ms: self
                .hub_recv_push_gap_over_1ms
                .saturating_sub(before.hub_recv_push_gap_over_1ms),
            hub_recv_push_gap_over_4ms: self
                .hub_recv_push_gap_over_4ms
                .saturating_sub(before.hub_recv_push_gap_over_4ms),
            hub_recv_push_gap_over_16ms: self
                .hub_recv_push_gap_over_16ms
                .saturating_sub(before.hub_recv_push_gap_over_16ms),
            hub_recv_wake_to_poll_gap_samples: self
                .hub_recv_wake_to_poll_gap_samples
                .saturating_sub(before.hub_recv_wake_to_poll_gap_samples),
            hub_recv_wake_to_poll_gap_ms_total: self
                .hub_recv_wake_to_poll_gap_ms_total
                .saturating_sub(before.hub_recv_wake_to_poll_gap_ms_total),
            hub_recv_wake_to_poll_gap_ms_max: self.hub_recv_wake_to_poll_gap_ms_max,
            hub_recv_wake_to_poll_gap_over_1ms: self
                .hub_recv_wake_to_poll_gap_over_1ms
                .saturating_sub(before.hub_recv_wake_to_poll_gap_over_1ms),
            hub_recv_wake_to_poll_gap_over_4ms: self
                .hub_recv_wake_to_poll_gap_over_4ms
                .saturating_sub(before.hub_recv_wake_to_poll_gap_over_4ms),
            hub_recv_wake_to_poll_gap_over_16ms: self
                .hub_recv_wake_to_poll_gap_over_16ms
                .saturating_sub(before.hub_recv_wake_to_poll_gap_over_16ms),
            hub_recv_pushes: self.hub_recv_pushes.saturating_sub(before.hub_recv_pushes),
            hub_recv_waker_sets: self
                .hub_recv_waker_sets
                .saturating_sub(before.hub_recv_waker_sets),
            hub_recv_wakes: self.hub_recv_wakes.saturating_sub(before.hub_recv_wakes),
            hub_outbound_push_gap_samples: self
                .hub_outbound_push_gap_samples
                .saturating_sub(before.hub_outbound_push_gap_samples),
            hub_outbound_push_gap_ms_total: self
                .hub_outbound_push_gap_ms_total
                .saturating_sub(before.hub_outbound_push_gap_ms_total),
            hub_outbound_push_gap_ms_max: self.hub_outbound_push_gap_ms_max,
            hub_outbound_push_gap_over_1ms: self
                .hub_outbound_push_gap_over_1ms
                .saturating_sub(before.hub_outbound_push_gap_over_1ms),
            hub_outbound_push_gap_over_4ms: self
                .hub_outbound_push_gap_over_4ms
                .saturating_sub(before.hub_outbound_push_gap_over_4ms),
            hub_outbound_push_gap_over_16ms: self
                .hub_outbound_push_gap_over_16ms
                .saturating_sub(before.hub_outbound_push_gap_over_16ms),
            hub_outbound_pushes: self
                .hub_outbound_pushes
                .saturating_sub(before.hub_outbound_pushes),
            hub_outbound_bytes: self
                .hub_outbound_bytes
                .saturating_sub(before.hub_outbound_bytes),
            hub_outbound_depth_max: self.hub_outbound_depth_max,
            hub_outbound_full: self
                .hub_outbound_full
                .saturating_sub(before.hub_outbound_full),
            hub_outbound_waker_sets: self
                .hub_outbound_waker_sets
                .saturating_sub(before.hub_outbound_waker_sets),
            hub_outbound_wakes: self
                .hub_outbound_wakes
                .saturating_sub(before.hub_outbound_wakes),
            hub_outbound_queue_delay_samples: self
                .hub_outbound_queue_delay_samples
                .saturating_sub(before.hub_outbound_queue_delay_samples),
            hub_outbound_queue_delay_ms_total: self
                .hub_outbound_queue_delay_ms_total
                .saturating_sub(before.hub_outbound_queue_delay_ms_total),
            hub_outbound_queue_delay_ms_max: self.hub_outbound_queue_delay_ms_max,
            hub_outbound_queue_delay_over_1ms: self
                .hub_outbound_queue_delay_over_1ms
                .saturating_sub(before.hub_outbound_queue_delay_over_1ms),
            hub_outbound_queue_delay_over_4ms: self
                .hub_outbound_queue_delay_over_4ms
                .saturating_sub(before.hub_outbound_queue_delay_over_4ms),
            hub_outbound_queue_delay_over_16ms: self
                .hub_outbound_queue_delay_over_16ms
                .saturating_sub(before.hub_outbound_queue_delay_over_16ms),
            hub_pop_matching_calls: self
                .hub_pop_matching_calls
                .saturating_sub(before.hub_pop_matching_calls),
            hub_pop_matching_hits: self
                .hub_pop_matching_hits
                .saturating_sub(before.hub_pop_matching_hits),
            hub_pop_matching_pending: self
                .hub_pop_matching_pending
                .saturating_sub(before.hub_pop_matching_pending),
            hub_pop_matching_scan_total: self
                .hub_pop_matching_scan_total
                .saturating_sub(before.hub_pop_matching_scan_total),
            hub_pop_matching_scan_max: self.hub_pop_matching_scan_max,
            data_channel_rx_message_gap_samples: self
                .data_channel_rx_message_gap_samples
                .saturating_sub(before.data_channel_rx_message_gap_samples),
            data_channel_rx_message_gap_ms_total: self
                .data_channel_rx_message_gap_ms_total
                .saturating_sub(before.data_channel_rx_message_gap_ms_total),
            data_channel_rx_message_gap_ms_max: self.data_channel_rx_message_gap_ms_max,
            data_channel_rx_message_gap_over_1ms: self
                .data_channel_rx_message_gap_over_1ms
                .saturating_sub(before.data_channel_rx_message_gap_over_1ms),
            data_channel_rx_message_gap_over_4ms: self
                .data_channel_rx_message_gap_over_4ms
                .saturating_sub(before.data_channel_rx_message_gap_over_4ms),
            data_channel_rx_message_gap_over_16ms: self
                .data_channel_rx_message_gap_over_16ms
                .saturating_sub(before.data_channel_rx_message_gap_over_16ms),
            data_channel_rx_messages: self
                .data_channel_rx_messages
                .saturating_sub(before.data_channel_rx_messages),
            data_channel_rx_frame_bytes: self
                .data_channel_rx_frame_bytes
                .saturating_sub(before.data_channel_rx_frame_bytes),
            data_channel_rx_payload_bytes: self
                .data_channel_rx_payload_bytes
                .saturating_sub(before.data_channel_rx_payload_bytes),
            data_channel_rx_invalid_frames: self
                .data_channel_rx_invalid_frames
                .saturating_sub(before.data_channel_rx_invalid_frames),
            data_channel_rx_wrong_session: self
                .data_channel_rx_wrong_session
                .saturating_sub(before.data_channel_rx_wrong_session),
            data_channel_rx_enqueue_failures: self
                .data_channel_rx_enqueue_failures
                .saturating_sub(before.data_channel_rx_enqueue_failures),
            data_channel_pump_pop_gap_samples: self
                .data_channel_pump_pop_gap_samples
                .saturating_sub(before.data_channel_pump_pop_gap_samples),
            data_channel_pump_pop_gap_ms_total: self
                .data_channel_pump_pop_gap_ms_total
                .saturating_sub(before.data_channel_pump_pop_gap_ms_total),
            data_channel_pump_pop_gap_ms_max: self.data_channel_pump_pop_gap_ms_max,
            data_channel_pump_pop_gap_over_1ms: self
                .data_channel_pump_pop_gap_over_1ms
                .saturating_sub(before.data_channel_pump_pop_gap_over_1ms),
            data_channel_pump_pop_gap_over_4ms: self
                .data_channel_pump_pop_gap_over_4ms
                .saturating_sub(before.data_channel_pump_pop_gap_over_4ms),
            data_channel_pump_pop_gap_over_16ms: self
                .data_channel_pump_pop_gap_over_16ms
                .saturating_sub(before.data_channel_pump_pop_gap_over_16ms),
            data_channel_pump_pops: self
                .data_channel_pump_pops
                .saturating_sub(before.data_channel_pump_pops),
            data_channel_send_gap_samples: self
                .data_channel_send_gap_samples
                .saturating_sub(before.data_channel_send_gap_samples),
            data_channel_send_gap_ms_total: self
                .data_channel_send_gap_ms_total
                .saturating_sub(before.data_channel_send_gap_ms_total),
            data_channel_send_gap_ms_max: self.data_channel_send_gap_ms_max,
            data_channel_send_gap_over_1ms: self
                .data_channel_send_gap_over_1ms
                .saturating_sub(before.data_channel_send_gap_over_1ms),
            data_channel_send_gap_over_4ms: self
                .data_channel_send_gap_over_4ms
                .saturating_sub(before.data_channel_send_gap_over_4ms),
            data_channel_send_gap_over_16ms: self
                .data_channel_send_gap_over_16ms
                .saturating_sub(before.data_channel_send_gap_over_16ms),
            data_channel_sent_messages: self
                .data_channel_sent_messages
                .saturating_sub(before.data_channel_sent_messages),
            data_channel_sent_bytes: self
                .data_channel_sent_bytes
                .saturating_sub(before.data_channel_sent_bytes),
            data_channel_buffered_amount_samples: self
                .data_channel_buffered_amount_samples
                .saturating_sub(before.data_channel_buffered_amount_samples),
            data_channel_buffered_amount_before_total: self
                .data_channel_buffered_amount_before_total
                .saturating_sub(before.data_channel_buffered_amount_before_total),
            data_channel_buffered_amount_after_total: self
                .data_channel_buffered_amount_after_total
                .saturating_sub(before.data_channel_buffered_amount_after_total),
            data_channel_buffered_amount_before_max: self.data_channel_buffered_amount_before_max,
            data_channel_buffered_amount_max: self.data_channel_buffered_amount_max,
            data_channel_buffered_amount_before_over_256k: self
                .data_channel_buffered_amount_before_over_256k
                .saturating_sub(before.data_channel_buffered_amount_before_over_256k),
            data_channel_buffered_amount_before_over_512k: self
                .data_channel_buffered_amount_before_over_512k
                .saturating_sub(before.data_channel_buffered_amount_before_over_512k),
            data_channel_buffered_amount_before_over_1m: self
                .data_channel_buffered_amount_before_over_1m
                .saturating_sub(before.data_channel_buffered_amount_before_over_1m),
            data_channel_buffered_amount_after_over_256k: self
                .data_channel_buffered_amount_after_over_256k
                .saturating_sub(before.data_channel_buffered_amount_after_over_256k),
            data_channel_buffered_amount_after_over_512k: self
                .data_channel_buffered_amount_after_over_512k
                .saturating_sub(before.data_channel_buffered_amount_after_over_512k),
            data_channel_buffered_amount_after_over_1m: self
                .data_channel_buffered_amount_after_over_1m
                .saturating_sub(before.data_channel_buffered_amount_after_over_1m),
            data_channel_send_failures: self
                .data_channel_send_failures
                .saturating_sub(before.data_channel_send_failures),
            endpoint_poll_recv_call_gap_samples: self
                .endpoint_poll_recv_call_gap_samples
                .saturating_sub(before.endpoint_poll_recv_call_gap_samples),
            endpoint_poll_recv_call_gap_ms_total: self
                .endpoint_poll_recv_call_gap_ms_total
                .saturating_sub(before.endpoint_poll_recv_call_gap_ms_total),
            endpoint_poll_recv_call_gap_ms_max: self.endpoint_poll_recv_call_gap_ms_max,
            endpoint_poll_recv_call_gap_over_1ms: self
                .endpoint_poll_recv_call_gap_over_1ms
                .saturating_sub(before.endpoint_poll_recv_call_gap_over_1ms),
            endpoint_poll_recv_call_gap_over_4ms: self
                .endpoint_poll_recv_call_gap_over_4ms
                .saturating_sub(before.endpoint_poll_recv_call_gap_over_4ms),
            endpoint_poll_recv_call_gap_over_16ms: self
                .endpoint_poll_recv_call_gap_over_16ms
                .saturating_sub(before.endpoint_poll_recv_call_gap_over_16ms),
            endpoint_poll_recv_calls: self
                .endpoint_poll_recv_calls
                .saturating_sub(before.endpoint_poll_recv_calls),
            endpoint_poll_recv_pending: self
                .endpoint_poll_recv_pending
                .saturating_sub(before.endpoint_poll_recv_pending),
            endpoint_poll_recv_ready: self
                .endpoint_poll_recv_ready
                .saturating_sub(before.endpoint_poll_recv_ready),
            endpoint_poll_recv_ready_packets: self
                .endpoint_poll_recv_ready_packets
                .saturating_sub(before.endpoint_poll_recv_ready_packets),
            endpoint_poll_recv_ready_packets_max: self.endpoint_poll_recv_ready_packets_max,
            endpoint_poll_recv_partial_pending: self
                .endpoint_poll_recv_partial_pending
                .saturating_sub(before.endpoint_poll_recv_partial_pending),
            endpoint_poll_recv_full: self
                .endpoint_poll_recv_full
                .saturating_sub(before.endpoint_poll_recv_full),
            endpoint_poll_recv_pending_frame_continuations: self
                .endpoint_poll_recv_pending_frame_continuations
                .saturating_sub(before.endpoint_poll_recv_pending_frame_continuations),
            endpoint_delivered_packets: self
                .endpoint_delivered_packets
                .saturating_sub(before.endpoint_delivered_packets),
            endpoint_delivered_payload_bytes: self
                .endpoint_delivered_payload_bytes
                .saturating_sub(before.endpoint_delivered_payload_bytes),
            endpoint_delivered_payload_max: self.endpoint_delivered_payload_max,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicUsize, Ordering as AtomicUsizeOrdering},
        task::{Context, Poll, Wake},
    };

    use super::*;

    #[derive(Debug)]
    struct NoopWake;

    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }

    #[derive(Debug)]
    struct CountWake(AtomicUsize);

    impl Wake for CountWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, AtomicUsizeOrdering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, AtomicUsizeOrdering::SeqCst);
        }
    }

    fn context() -> Context<'static> {
        let waker = std::task::Waker::from(Arc::new(NoopWake));
        Context::from_waker(Box::leak(Box::new(waker)))
    }

    fn counted_context() -> (Context<'static>, Arc<CountWake>) {
        let wake = Arc::new(CountWake(AtomicUsize::new(0)));
        let waker = std::task::Waker::from(wake.clone());
        (Context::from_waker(Box::leak(Box::new(waker))), wake)
    }

    fn encoded(frame: &WebRtcPacketFrame) -> Vec<u8> {
        frame
            .encode(crate::core::frame::DEFAULT_MAX_PAYLOAD_LEN)
            .unwrap()
    }

    fn destination(session_id: [u8; 16]) -> CustomAddr {
        WebRtcAddr::session(iroh::SecretKey::generate().public(), session_id).to_custom_addr()
    }

    fn outbound_packet(frame: &WebRtcPacketFrame) -> OutboundPacket {
        OutboundPacket {
            destination: destination(frame.session_id),
            bytes: encoded(frame),
        }
    }

    #[test]
    fn receive_queue_enforces_capacity() {
        let hub = SessionHub::with_capacities(1, 1);
        let source = CustomAddr::from_parts(1, &[1]);
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [0; 16],
            segment_size: None,
            payload: Vec::new(),
        };

        hub.push_received(QueuedPacket {
            source: source.clone(),
            frame: frame.clone(),
        })
        .unwrap();

        assert!(matches!(
            hub.push_received(QueuedPacket { source, frame }),
            Err(Error::SendQueueFull)
        ));
    }

    #[test]
    fn close_wakes_as_error() {
        let hub = SessionHub::default();
        hub.close();
        let mut cx = context();
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [2; 16],
            segment_size: None,
            payload: b"closed".to_vec(),
        };

        assert!(matches!(hub.pop_received(&mut cx), Poll::Ready(Err(_))));
        assert!(matches!(
            hub.poll_push_outbound(&mut cx, outbound_packet(&frame)),
            Poll::Ready(Err(Error::SessionClosed))
        ));
    }

    #[test]
    fn outbound_route_can_be_drained_by_registered_receiver() {
        let hub = SessionHub::with_capacities(1, 2);
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [2; 16],
            segment_size: None,
            payload: b"hello".to_vec(),
        };
        let mut receiver = hub.register_outbound_session(frame.session_id);
        let packet = outbound_packet(&frame);
        let destination = packet.destination.clone();

        let mut cx = context();
        assert!(matches!(
            hub.poll_push_outbound(&mut cx, packet),
            Poll::Ready(Ok(()))
        ));

        let drained = receiver.try_recv().unwrap();
        assert_eq!(drained.destination, destination);
        assert_eq!(drained.bytes, encoded(&frame));
    }

    #[test]
    fn outbound_route_preserves_per_session_order() {
        let hub = SessionHub::with_capacities(1, 2);
        let first_frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [2; 16],
            segment_size: None,
            payload: b"first".to_vec(),
        };
        let second_frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [2; 16],
            segment_size: None,
            payload: b"second".to_vec(),
        };
        let mut receiver = hub.register_outbound_session(first_frame.session_id);
        let mut cx = context();

        assert!(matches!(
            hub.poll_push_outbound(&mut cx, outbound_packet(&first_frame)),
            Poll::Ready(Ok(()))
        ));
        assert!(matches!(
            hub.poll_push_outbound(&mut cx, outbound_packet(&second_frame)),
            Poll::Ready(Ok(()))
        ));

        let first = receiver.try_recv().unwrap();
        let second = receiver.try_recv().unwrap();
        assert_eq!(first.bytes, encoded(&first_frame));
        assert_eq!(second.bytes, encoded(&second_frame));
    }

    #[test]
    fn outbound_route_full_returns_pending_until_receiver_frees_capacity() {
        let hub = SessionHub::with_capacities(1, 1);
        let first_frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [2; 16],
            segment_size: None,
            payload: b"first".to_vec(),
        };
        let second_frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [2; 16],
            segment_size: None,
            payload: b"second".to_vec(),
        };
        let mut receiver = hub.register_outbound_session(first_frame.session_id);
        let (mut cx, wake) = counted_context();
        let second_packet = outbound_packet(&second_frame);

        assert!(matches!(
            hub.poll_push_outbound(&mut cx, outbound_packet(&first_frame)),
            Poll::Ready(Ok(()))
        ));
        assert!(matches!(
            hub.poll_push_outbound(&mut cx, second_packet.clone()),
            Poll::Pending
        ));

        assert_eq!(receiver.try_recv().unwrap().bytes, encoded(&first_frame));
        assert!(wake.0.load(AtomicUsizeOrdering::SeqCst) > 0);
        assert!(matches!(
            hub.poll_push_outbound(&mut cx, second_packet),
            Poll::Ready(Ok(()))
        ));
        assert_eq!(receiver.try_recv().unwrap().bytes, encoded(&second_frame));
    }

    #[test]
    fn unregister_outbound_route_wakes_pending_sender() {
        let hub = SessionHub::with_capacities(1, 1);
        let first_frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [2; 16],
            segment_size: None,
            payload: b"first".to_vec(),
        };
        let second_frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [2; 16],
            segment_size: None,
            payload: b"second".to_vec(),
        };
        let _receiver = hub.register_outbound_session(first_frame.session_id);
        let (mut cx, wake) = counted_context();
        let second_packet = outbound_packet(&second_frame);

        assert!(matches!(
            hub.poll_push_outbound(&mut cx, outbound_packet(&first_frame)),
            Poll::Ready(Ok(()))
        ));
        assert!(matches!(
            hub.poll_push_outbound(&mut cx, second_packet.clone()),
            Poll::Pending
        ));

        assert!(hub.unregister_outbound_session(first_frame.session_id));
        assert!(wake.0.load(AtomicUsizeOrdering::SeqCst) > 0);
        assert!(matches!(
            hub.poll_push_outbound(&mut cx, second_packet),
            Poll::Ready(Err(Error::SessionClosed))
        ));
    }

    #[test]
    fn missing_or_unregistered_outbound_route_errors() {
        let hub = SessionHub::with_capacities(1, 1);
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [1; 16],
            segment_size: None,
            payload: b"missing".to_vec(),
        };
        let mut cx = context();

        assert!(matches!(
            hub.poll_push_outbound(&mut cx, outbound_packet(&frame)),
            Poll::Ready(Err(Error::SessionClosed))
        ));

        let _receiver = hub.register_outbound_session(frame.session_id);
        assert!(hub.unregister_outbound_session(frame.session_id));
        assert!(matches!(
            hub.poll_push_outbound(&mut cx, outbound_packet(&frame)),
            Poll::Ready(Err(Error::SessionClosed))
        ));
    }

    #[test]
    fn local_ice_queue_enforces_capacity_and_order() {
        let queue = LocalIceQueue::with_capacity(2);
        let candidate = LocalIceEvent::Candidate(WebRtcIceCandidate {
            candidate: "candidate:1 1 UDP 1 127.0.0.1 9 typ host".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        });

        queue.push(candidate.clone()).unwrap();
        queue.push(LocalIceEvent::EndOfCandidates).unwrap();
        assert!(matches!(
            queue.push(LocalIceEvent::Candidate(WebRtcIceCandidate {
                candidate: "candidate:2 1 UDP 1 127.0.0.1 10 typ host".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: Some(0),
            })),
            Err(Error::SendQueueFull)
        ));

        let mut cx = context();
        match queue.poll_next(&mut cx) {
            Poll::Ready(Ok(event)) => assert_eq!(event, candidate),
            other => panic!("unexpected poll result: {other:?}"),
        }
        match queue.poll_next(&mut cx) {
            Poll::Ready(Ok(event)) => assert_eq!(event, LocalIceEvent::EndOfCandidates),
            other => panic!("unexpected poll result: {other:?}"),
        }
        assert!(matches!(queue.poll_next(&mut cx), Poll::Pending));
    }

    #[test]
    fn local_ice_queue_preserves_end_of_candidates_when_full() {
        let queue = LocalIceQueue::with_capacity(1);
        queue
            .push(LocalIceEvent::Candidate(WebRtcIceCandidate {
                candidate: "candidate:1 1 UDP 1 127.0.0.1 9 typ host".into(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: Some(0),
            }))
            .unwrap();
        queue.push(LocalIceEvent::EndOfCandidates).unwrap();

        let mut cx = context();
        match queue.poll_next(&mut cx) {
            Poll::Ready(Ok(event)) => assert_eq!(event, LocalIceEvent::EndOfCandidates),
            other => panic!("unexpected poll result: {other:?}"),
        }
    }
}
