use std::{
    io,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use iroh::endpoint::transports::{Addr, CustomEndpoint, CustomSender};
use iroh_base::CustomAddr;

use crate::core::{hub::SessionHub, sender::WebRtcSender};

#[derive(Debug)]
pub struct WebRtcEndpoint {
    local_addrs: n0_watcher::Watchable<Vec<CustomAddr>>,
    hub: SessionHub,
    sender: Arc<WebRtcSender>,
    pending: Mutex<Option<PendingPacket>>,
    max_transmit_segments: NonZeroUsize,
}

#[derive(Debug)]
struct PendingPacket {
    packet: crate::core::hub::QueuedPacket,
    offset: usize,
}

impl WebRtcEndpoint {
    pub fn new(
        local_addrs: n0_watcher::Watchable<Vec<CustomAddr>>,
        hub: SessionHub,
        max_payload_len: usize,
        max_transmit_segments: NonZeroUsize,
    ) -> Self {
        let advertised_addrs = local_addrs.get();
        tracing::trace!(
            target: "iroh_webrtc_transport::endpoint",
            local_addrs = ?advertised_addrs,
            max_payload_len,
            max_transmit_segments = max_transmit_segments.get(),
            "created WebRTC custom endpoint"
        );
        Self {
            local_addrs,
            hub: hub.clone(),
            sender: Arc::new(WebRtcSender::new(hub, max_payload_len)),
            pending: Mutex::new(None),
            max_transmit_segments,
        }
    }
}

impl CustomEndpoint for WebRtcEndpoint {
    fn watch_local_addrs(&self) -> n0_watcher::Direct<Vec<CustomAddr>> {
        tracing::trace!(
            target: "iroh_webrtc_transport::endpoint",
            local_addrs = ?self.local_addrs.get(),
            "watching WebRTC custom endpoint local addresses"
        );
        self.local_addrs.watch()
    }

    fn create_sender(&self) -> Arc<dyn CustomSender> {
        tracing::trace!(
            target: "iroh_webrtc_transport::endpoint",
            local_addrs = ?self.local_addrs.get(),
            "creating WebRTC custom sender"
        );
        self.sender.clone()
    }

    fn poll_recv(
        &mut self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        metas: &mut [noq_udp::RecvMeta],
        source_addrs: &mut [Addr],
    ) -> Poll<io::Result<usize>> {
        debug_assert_eq!(bufs.len(), metas.len());
        debug_assert_eq!(bufs.len(), source_addrs.len());
        if bufs.is_empty() {
            return Poll::Ready(Ok(0));
        }

        self.hub.record_endpoint_poll_recv_call();
        let mut count = 0;
        loop {
            if count == bufs.len() {
                self.hub.record_endpoint_poll_recv_ready(count, true);
                return Poll::Ready(Ok(count));
            }

            let pending = {
                let mut pending = self.pending.lock().expect("pending mutex poisoned");
                pending.take()
            };
            let continued_pending_frame = pending.is_some();
            let mut pending = match pending {
                Some(pending) => pending,
                None => match self.hub.pop_received(cx) {
                    Poll::Ready(Ok(packet)) => PendingPacket { packet, offset: 0 },
                    Poll::Ready(Err(err)) if count > 0 => {
                        tracing::debug!(%err, "returning partial WebRTC packet batch");
                        self.hub.record_endpoint_poll_recv_ready(count, false);
                        return Poll::Ready(Ok(count));
                    }
                    Poll::Ready(Err(err)) => return Poll::Ready(Err(err)),
                    Poll::Pending if count > 0 => {
                        self.hub.record_endpoint_poll_recv_partial_pending();
                        self.hub.record_endpoint_poll_recv_ready(count, false);
                        return Poll::Ready(Ok(count));
                    }
                    Poll::Pending => {
                        self.hub.record_endpoint_poll_recv_pending();
                        return Poll::Pending;
                    }
                },
            };
            if continued_pending_frame {
                self.hub
                    .record_endpoint_poll_recv_pending_frame_continuation();
            }

            let payload_len = pending.packet.frame.payload.len();
            if payload_len == 0 {
                tracing::debug!(
                    target: "iroh_webrtc_transport::endpoint",
                    source = ?pending.packet.source,
                    session_id = ?pending.packet.frame.session_id,
                    "dropping empty WebRTC packet frame"
                );
                continue;
            }
            if pending.offset >= payload_len {
                continue;
            }

            let stride = pending.packet.frame.segment_size.unwrap_or(payload_len);
            if pending.offset == 0 && pending.packet.frame.segment_size.is_some() {
                if payload_len <= bufs[count].len() {
                    bufs[count][..payload_len].copy_from_slice(&pending.packet.frame.payload);
                    let mut meta = noq_udp::RecvMeta::default();
                    meta.len = payload_len;
                    meta.stride = stride;
                    metas[count] = meta;
                    source_addrs[count] = Addr::Custom(pending.packet.source);
                    self.hub.record_endpoint_delivery(
                        payload_len,
                        payload_len.div_ceil(stride),
                        stride,
                    );
                    tracing::trace!(
                        target: "iroh_webrtc_transport::endpoint",
                        packet_index = count,
                        payload_bytes = payload_len,
                        frame_segment_size = stride,
                        "delivered segmented WebRTC custom packet batch to Iroh"
                    );
                    count += 1;
                    continue;
                }
            }

            let segment_len = stride.min(payload_len - pending.offset);
            if segment_len > bufs[count].len() {
                let mut stored = self.pending.lock().expect("pending mutex poisoned");
                *stored = Some(pending);
                if count > 0 {
                    self.hub.record_endpoint_poll_recv_ready(count, false);
                    return Poll::Ready(Ok(count));
                }
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "receive buffer too small for WebRTC frame payload",
                )));
            }

            let start = pending.offset;
            let end = start + segment_len;
            bufs[count][..segment_len].copy_from_slice(&pending.packet.frame.payload[start..end]);
            let mut meta = noq_udp::RecvMeta::default();
            meta.len = segment_len;
            meta.stride = segment_len;
            metas[count] = meta;
            source_addrs[count] = Addr::Custom(pending.packet.source.clone());
            self.hub
                .record_endpoint_delivery(segment_len, 1, segment_len);
            tracing::trace!(
                target: "iroh_webrtc_transport::endpoint",
                packet_index = count,
                payload_bytes = segment_len,
                frame_payload_bytes = payload_len,
                frame_segment_size = ?pending.packet.frame.segment_size,
                "delivered WebRTC custom packet to Iroh"
            );
            pending.offset = end;
            if pending.offset < payload_len {
                let mut stored = self.pending.lock().expect("pending mutex poisoned");
                *stored = Some(pending);
            }
            count += 1;
        }
    }

    fn max_transmit_segments(&self) -> NonZeroUsize {
        self.max_transmit_segments
    }
}
