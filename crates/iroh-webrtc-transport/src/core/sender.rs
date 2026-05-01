use std::{
    io,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use iroh::endpoint::transports::{CustomSender, Transmit};
use iroh_base::CustomAddr;

use crate::{
    core::{
        addr::{WebRtcAddr, WebRtcAddrKind},
        frame::WebRtcPacketFrame,
        hub::{OutboundPacket, SessionHub},
    },
    error::Error,
};

#[derive(Debug)]
pub struct WebRtcSender {
    hub: SessionHub,
    validation_checks: Arc<Mutex<u64>>,
    max_payload_len: usize,
}

impl WebRtcSender {
    pub fn new(hub: SessionHub, max_payload_len: usize) -> Self {
        Self {
            hub,
            validation_checks: Arc::new(Mutex::new(0)),
            max_payload_len,
        }
    }
}

impl CustomSender for WebRtcSender {
    fn is_valid_send_addr(&self, addr: &CustomAddr) -> bool {
        let parsed = WebRtcAddr::from_custom_addr(addr);
        let valid = matches!(
            parsed,
            Ok(WebRtcAddr {
                kind: WebRtcAddrKind::Session { .. },
                ..
            })
        );
        let check_count = {
            let mut count = self
                .validation_checks
                .lock()
                .expect("validation count mutex poisoned");
            *count += 1;
            *count
        };
        if !valid {
            tracing::debug!(
                target: "iroh_webrtc_transport::sender",
                check_count,
                valid,
                parsed_addr = ?parsed,
                destination = ?addr,
                "validated WebRTC custom send address"
            );
        } else {
            tracing::trace!(
                target: "iroh_webrtc_transport::sender",
                check_count,
                parsed_addr = ?parsed,
                destination = ?addr,
                "validated WebRTC custom send address"
            );
        }
        valid
    }

    fn poll_send(
        &self,
        cx: &mut Context,
        dst: &CustomAddr,
        transmit: &Transmit<'_>,
    ) -> Poll<io::Result<()>> {
        self.hub.record_sender_poll_send_call();
        let parsed = match WebRtcAddr::from_custom_addr(dst) {
            Ok(
                parsed @ WebRtcAddr {
                    kind: WebRtcAddrKind::Session { .. },
                    ..
                },
            ) => parsed,
            Ok(_) => {
                tracing::debug!(
                    target: "iroh_webrtc_transport::sender",
                    destination = ?dst,
                    "rejecting outbound WebRTC send to capability address"
                );
                return Poll::Ready(Err(io::Error::from(Error::InvalidAddr(
                    "cannot send to capability address",
                ))));
            }
            Err(err) => {
                tracing::debug!(
                    target: "iroh_webrtc_transport::sender",
                    destination = ?dst,
                    %err,
                    "rejecting outbound WebRTC send to invalid address"
                );
                return Poll::Ready(Err(io::Error::from(err)));
            }
        };
        let WebRtcAddrKind::Session { session_id } = parsed.kind else {
            unreachable!("session address checked above");
        };

        let datagrams = transmit
            .segment_size
            .filter(|segment_size| *segment_size > 0)
            .map(|segment_size| transmit.contents.len().div_ceil(segment_size))
            .unwrap_or(1);
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id,
            segment_size: transmit.segment_size,
            payload: transmit.contents.to_vec(),
        };
        let bytes = match frame.encode(self.max_payload_len) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.hub.record_sender_encode_failure();
                tracing::debug!(
                    target: "iroh_webrtc_transport::sender",
                    destination = ?dst,
                    session_id = ?session_id,
                    payload_bytes = transmit.contents.len(),
                    %err,
                    "failed to encode outbound WebRTC datagram"
                );
                return Poll::Ready(Err(io::Error::from(err)));
            }
        };

        let frame_bytes = bytes.len();
        let result = self.hub.poll_push_outbound(
            cx,
            OutboundPacket {
                destination: dst.clone(),
                bytes,
            },
        );
        match result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) => {
                self.hub
                    .record_sender_transmit(transmit.contents.len(), frame_bytes, datagrams);
                tracing::trace!(
                    target: "iroh_webrtc_transport::sender",
                    destination = ?dst,
                    session_id = ?session_id,
                    payload_bytes = transmit.contents.len(),
                    frame_bytes,
                    datagrams,
                    segment_size = ?transmit.segment_size,
                    "queued outbound WebRTC datagram"
                );
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(err)) => {
                self.hub.record_sender_queue_failure();
                tracing::debug!(
                    target: "iroh_webrtc_transport::sender",
                    destination = ?dst,
                    session_id = ?session_id,
                    payload_bytes = transmit.contents.len(),
                    frame_bytes,
                    %err,
                    "failed to queue outbound WebRTC datagram"
                );
                Poll::Ready(Err(io::Error::from(err)))
            }
        }
    }
}
