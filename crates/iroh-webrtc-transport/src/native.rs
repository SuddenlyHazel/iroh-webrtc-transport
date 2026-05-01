//! Native/server WebRTC entry points.
//!
//! Use this module on non-wasm targets that need to answer or initiate the
//! WebRTC side of the bootstrap flow. The native backend owns an
//! `RTCPeerConnection`, accepts or creates the DataChannel, and attaches packet
//! I/O to a shared `SessionHub` for Iroh's custom transport.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use bytes::Bytes;
use iroh_base::CustomAddr;
use n0_future::task;
use tokio::sync::{Notify, oneshot};
use webrtc::{
    api::APIBuilder,
    data_channel::{
        RTCDataChannel, data_channel_init::RTCDataChannelInit,
        data_channel_message::DataChannelMessage,
    },
    ice_transport::{ice_candidate::RTCIceCandidateInit, ice_server::RTCIceServer},
    peer_connection::{
        RTCPeerConnection, configuration::RTCConfiguration, offer_answer_options::RTCOfferOptions,
        sdp::session_description::RTCSessionDescription,
    },
};

use crate::{
    config::WebRtcSessionConfig,
    core::{
        frame::WebRtcPacketFrame,
        hub::{LocalIceQueue, OutboundPacketReceiver, QueuedPacket, SessionHub},
    },
    error::{Error, Result},
};

pub use crate::core::{
    addr::WebRtcAddr,
    hub::{LocalIceEvent, WebRtcIceCandidate},
    signaling::WEBRTC_DATA_CHANNEL_LABEL,
};

/// Native-facing alias for the shared per-session WebRTC configuration.
pub type NativeWebRtcSessionConfig = WebRtcSessionConfig;

/// Native WebRTC peer connection bound to one Iroh custom transport session.
#[derive(Debug, Clone)]
pub struct NativeWebRtcSession {
    peer: Arc<RTCPeerConnection>,
    local_ice: LocalIceQueue,
}

impl NativeWebRtcSession {
    /// Create an initiating native session and attach an outbound DataChannel.
    pub async fn new_offerer_with_config(
        hub: SessionHub,
        remote_addr: CustomAddr,
        session_id: [u8; 16],
        data_channel_label: &str,
        config: WebRtcSessionConfig,
    ) -> Result<Self> {
        let session = Self::new_with_config(config.clone()).await?;
        let channel = session
            .peer
            .create_data_channel(
                data_channel_label,
                Some(RTCDataChannelInit {
                    ordered: Some(false),
                    max_retransmits: Some(0),
                    ..Default::default()
                }),
            )
            .await
            .map_err(native_error)?;
        attach_data_channel(channel, hub, remote_addr, session_id, config).await;
        Ok(session)
    }

    /// Create an answering native session that accepts the remote DataChannel.
    pub async fn new_answerer_with_config(
        hub: SessionHub,
        remote_addr: CustomAddr,
        session_id: [u8; 16],
        config: WebRtcSessionConfig,
    ) -> Result<Self> {
        let session = Self::new_with_config(config.clone()).await?;
        let data_hub = hub.clone();
        let data_remote_addr = remote_addr.clone();
        session.peer.on_data_channel(Box::new(move |channel| {
            let data_hub = data_hub.clone();
            let data_remote_addr = data_remote_addr.clone();
            let config = config.clone();
            Box::pin(async move {
                attach_data_channel(channel, data_hub, data_remote_addr, session_id, config).await;
            })
        }));
        Ok(session)
    }

    async fn new_with_config(config: WebRtcSessionConfig) -> Result<Self> {
        config.validate()?;
        let api = APIBuilder::new().build();
        let peer = Arc::new(
            api.new_peer_connection(RTCConfiguration {
                ice_servers: config
                    .ice
                    .stun_urls
                    .iter()
                    .map(|url| RTCIceServer {
                        urls: vec![url.clone()],
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })
            .await
            .map_err(native_error)?,
        );
        let local_ice = LocalIceQueue::with_capacity(config.local_ice_queue_capacity);

        let ice_queue = local_ice.clone();
        peer.on_ice_candidate(Box::new(move |candidate| {
            let ice_queue = ice_queue.clone();
            Box::pin(async move {
                let event = match candidate {
                    Some(candidate) => match candidate.to_json() {
                        Ok(init) => LocalIceEvent::Candidate(WebRtcIceCandidate {
                            candidate: init.candidate,
                            sdp_mid: normalize_sdp_mid(init.sdp_mid),
                            sdp_mline_index: init.sdp_mline_index,
                        }),
                        Err(err) => {
                            tracing::debug!(%err, "dropping native WebRTC ICE candidate");
                            return;
                        }
                    },
                    None => LocalIceEvent::EndOfCandidates,
                };
                if let Err(err) = ice_queue.push(event) {
                    tracing::debug!(%err, "failed to queue native WebRTC ICE candidate");
                }
            })
        }));

        peer.on_peer_connection_state_change(Box::new(move |state| {
            Box::pin(async move {
                tracing::info!(state = %state, "native WebRTC peer connection state changed");
            })
        }));

        Ok(Self { peer, local_ice })
    }

    /// Create and apply a WebRTC offer.
    pub async fn create_offer(&self) -> Result<String> {
        self.create_offer_with_ice_restart(false).await
    }

    async fn create_offer_with_ice_restart(&self, ice_restart: bool) -> Result<String> {
        let options = RTCOfferOptions {
            ice_restart,
            ..Default::default()
        };
        let offer = self
            .peer
            .create_offer(Some(options))
            .await
            .map_err(native_error)?;
        let sdp = offer.sdp.clone();
        self.peer
            .set_local_description(offer)
            .await
            .map_err(native_error)?;
        Ok(normalize_sdp(sdp))
    }

    /// Apply a remote WebRTC answer SDP.
    pub async fn apply_answer(&self, sdp: String) -> Result<()> {
        self.peer
            .set_remote_description(
                RTCSessionDescription::answer(normalize_sdp(sdp)).map_err(native_error)?,
            )
            .await
            .map_err(native_error)
    }

    /// Apply a remote WebRTC offer SDP.
    pub async fn apply_offer(&self, sdp: String) -> Result<()> {
        self.peer
            .set_remote_description(
                RTCSessionDescription::offer(normalize_sdp(sdp)).map_err(native_error)?,
            )
            .await
            .map_err(native_error)
    }

    /// Create and apply a WebRTC answer.
    pub async fn create_answer(&self) -> Result<String> {
        let answer = self.peer.create_answer(None).await.map_err(native_error)?;
        let sdp = answer.sdp.clone();
        self.peer
            .set_local_description(answer)
            .await
            .map_err(native_error)?;
        Ok(normalize_sdp(sdp))
    }

    /// Add one remote ICE candidate.
    pub async fn add_ice_candidate(&self, candidate: WebRtcIceCandidate) -> Result<()> {
        self.peer
            .add_ice_candidate(RTCIceCandidateInit {
                candidate: candidate.candidate,
                sdp_mid: candidate.sdp_mid,
                sdp_mline_index: candidate.sdp_mline_index,
                username_fragment: None,
            })
            .await
            .map_err(native_error)
    }

    /// Signal that the remote peer has no more ICE candidates.
    pub async fn add_end_of_candidates(&self) -> Result<()> {
        self.peer
            .add_ice_candidate(RTCIceCandidateInit {
                candidate: String::new(),
                sdp_mid: Some("0".into()),
                sdp_mline_index: Some(0),
                username_fragment: None,
            })
            .await
            .map_err(native_error)
    }

    /// Wait for the next locally gathered ICE event.
    pub async fn next_local_ice(&self) -> Result<LocalIceEvent> {
        self.local_ice.next().await
    }

    /// Close the native WebRTC peer connection and local ICE queue.
    pub async fn close(&self) {
        self.local_ice.close();
        if let Err(err) = self.peer.close().await {
            tracing::debug!(%err, "failed to close native WebRTC peer connection");
        }
    }
}

async fn attach_data_channel(
    channel: Arc<RTCDataChannel>,
    hub: SessionHub,
    remote_addr: CustomAddr,
    session_id: [u8; 16],
    config: WebRtcSessionConfig,
) {
    tracing::info!(
        label = channel.label(),
        "accepted native WebRTC data channel"
    );
    // `on_open` is the normal path, but a channel handed to this function may
    // already be open. Keep one outbound pump per attached DataChannel.
    let pump_started = Arc::new(AtomicBool::new(false));
    let outbound_rx = Arc::new(Mutex::new(Some(hub.register_outbound_session(session_id))));
    let close_notify = Arc::new(Notify::new());
    let open_channel = channel.clone();
    let open_hub = hub.clone();
    let open_addr = remote_addr.clone();
    let open_started = pump_started.clone();
    let open_outbound_rx = outbound_rx.clone();
    let open_config = config.clone();
    let open_close_notify = close_notify.clone();
    channel.on_open(Box::new(move || {
        let open_channel = open_channel.clone();
        let open_hub = open_hub.clone();
        let open_addr = open_addr.clone();
        let open_started = open_started.clone();
        let open_outbound_rx = open_outbound_rx.clone();
        let open_config = open_config.clone();
        let open_close_notify = open_close_notify.clone();
        Box::pin(async move {
            if open_started.swap(true, Ordering::SeqCst) {
                return;
            }
            let Some(outbound_rx) = open_outbound_rx
                .lock()
                .expect("outbound receiver mutex poisoned")
                .take()
            else {
                return;
            };
            tracing::info!("native WebRTC data channel open");
            task::spawn(async move {
                pump_outbound_frames(
                    open_channel,
                    open_hub,
                    outbound_rx,
                    open_addr,
                    open_config,
                    open_close_notify,
                )
                .await;
            });
        })
    }));
    if channel_is_open(&channel) && !pump_started.swap(true, Ordering::SeqCst) {
        let open_channel = channel.clone();
        let open_hub = hub.clone();
        let open_addr = remote_addr.clone();
        let config = config.clone();
        let close_notify = close_notify.clone();
        let outbound_rx = outbound_rx
            .lock()
            .expect("outbound receiver mutex poisoned")
            .take();
        if let Some(outbound_rx) = outbound_rx {
            task::spawn(async move {
                pump_outbound_frames(
                    open_channel,
                    open_hub,
                    outbound_rx,
                    open_addr,
                    config,
                    close_notify,
                )
                .await;
            });
        }
    }

    let message_hub = hub.clone();
    let message_addr = remote_addr.clone();
    let message_frame_config = config.frame;
    channel.on_message(Box::new(move |message: DataChannelMessage| {
        let message_hub = message_hub.clone();
        let message_addr = message_addr.clone();
        Box::pin(async move {
            if message.is_string {
                tracing::debug!("dropping string WebRTC data channel message");
                return;
            }
            match WebRtcPacketFrame::decode(&message.data, message_frame_config.max_payload_len) {
                Ok(frame) => {
                    if frame.session_id != session_id {
                        tracing::debug!("dropping native WebRTC packet frame for wrong session");
                        return;
                    }
                    if let Err(err) = message_hub.push_received(QueuedPacket {
                        source: message_addr.clone(),
                        frame,
                    }) {
                        tracing::debug!(%err, "failed to enqueue native WebRTC packet frame");
                    }
                }
                Err(err) => {
                    tracing::debug!(%err, "dropping invalid native WebRTC packet frame");
                }
            }
        })
    }));

    let error_notify = close_notify.clone();
    channel.on_error(Box::new(move |err| {
        let error_notify = error_notify.clone();
        Box::pin(async move {
            error_notify.notify_waiters();
            tracing::debug!(%err, "native WebRTC data channel error");
        })
    }));

    channel.on_close(Box::new(move || {
        let close_hub = hub.clone();
        let close_notify = close_notify.clone();
        Box::pin(async move {
            close_hub.unregister_outbound_session(session_id);
            close_notify.notify_waiters();
            tracing::info!("native WebRTC data channel closed");
        })
    }));
}

async fn pump_outbound_frames(
    channel: Arc<RTCDataChannel>,
    hub: SessionHub,
    mut outbound_rx: OutboundPacketReceiver,
    destination: CustomAddr,
    config: WebRtcSessionConfig,
    close_notify: Arc<Notify>,
) {
    let buffered_amount_low_threshold = config.data_channel.buffered_amount_low_threshold;
    let buffered_amount_high_threshold = config.data_channel.buffered_amount_high_threshold;
    channel
        .set_buffered_amount_low_threshold(buffered_amount_low_threshold)
        .await;
    loop {
        let packet = match outbound_rx.recv().await {
            Some(packet) => packet,
            None => {
                tracing::debug!("stopping native WebRTC outbound pump because route closed");
                return;
            }
        };

        if !channel_is_open(&channel) {
            hub.unregister_outbound_for(&destination);
            tracing::debug!("stopping native WebRTC outbound pump for closed channel");
            return;
        }

        if channel
            .buffered_amount()
            .await
            .saturating_add(packet.bytes.len())
            > buffered_amount_high_threshold
            && !wait_for_native_data_channel_capacity(
                &channel,
                buffered_amount_low_threshold,
                buffered_amount_high_threshold,
                &close_notify,
            )
            .await
        {
            hub.unregister_outbound_for(&destination);
            let buffered_amount = channel.buffered_amount().await;
            tracing::debug!(
                buffered_amount,
                buffered_amount_low_threshold,
                buffered_amount_high_threshold,
                "stopping native WebRTC outbound pump while waiting for capacity"
            );
            return;
        }

        if let Err(err) = channel.send(&Bytes::from(packet.bytes)).await {
            tracing::debug!(%err, "failed to send native WebRTC data channel frame");
            hub.unregister_outbound_for(&destination);
            return;
        }
    }
}

async fn wait_for_native_data_channel_capacity(
    channel: &Arc<RTCDataChannel>,
    low_threshold: usize,
    high_threshold: usize,
    close_notify: &Arc<Notify>,
) -> bool {
    if channel.buffered_amount().await <= low_threshold {
        return true;
    }

    let buffered_amount = channel.buffered_amount().await;
    tracing::trace!(
        buffered_amount,
        buffered_amount_low_threshold = low_threshold,
        buffered_amount_high_threshold = high_threshold,
        "waiting for native WebRTC data channel buffered amount to drain"
    );

    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let low_tx = tx.clone();
    channel
        .on_buffered_amount_low(Box::new(move || {
            let low_tx = low_tx.clone();
            Box::pin(async move {
                if let Some(tx) = low_tx
                    .lock()
                    .expect("buffered amount low sender mutex poisoned")
                    .take()
                {
                    let _ = tx.send(());
                }
            })
        }))
        .await;

    if channel.buffered_amount().await <= low_threshold {
        if let Some(tx) = tx
            .lock()
            .expect("buffered amount low sender mutex poisoned")
            .take()
        {
            let _ = tx.send(());
        }
    }

    tokio::select! {
        _ = rx => true,
        _ = close_notify.notified() => false,
    }
}

fn channel_is_open(channel: &RTCDataChannel) -> bool {
    matches!(
        channel.ready_state(),
        webrtc::data_channel::data_channel_state::RTCDataChannelState::Open
    )
}

fn normalize_sdp_mid(mid: Option<String>) -> Option<String> {
    match mid.as_deref() {
        Some("") => Some("0".into()),
        _ => mid,
    }
}

fn normalize_sdp(sdp: String) -> String {
    let normalized = sdp.replace("\r\n", "\n").replace('\n', "\r\n");
    if normalized.ends_with("\r\n") {
        normalized
    } else {
        format!("{normalized}\r\n")
    }
}

fn native_error(err: impl std::fmt::Display) -> Error {
    Error::WebRtc(err.to_string())
}
