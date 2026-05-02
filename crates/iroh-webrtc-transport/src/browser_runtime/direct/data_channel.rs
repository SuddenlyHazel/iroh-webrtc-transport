use std::{cell::RefCell, rc::Rc};

use crate::core::{
    addr::WebRtcAddr,
    frame::WebRtcPacketFrame,
    hub::{OutboundPacketReceiver, QueuedPacket, SessionHub},
};
use iroh_base::CustomAddr;
use tokio::sync::oneshot;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{Event, MessageEvent, RtcDataChannel, RtcDataChannelState, RtcDataChannelType};

use super::js_boundary::{js_array_buffer_like_to_vec, js_error_message, runtime_error_to_js};
use super::rtc_registry::{self, BrowserRtcControl};
use super::*;

pub(super) fn attach_data_channel_from_main_channel(
    core: &Rc<BrowserRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    session_key: String,
    source: DataChannelSource,
    channel: RtcDataChannel,
) -> Result<(), JsValue> {
    let session =
        BrowserSessionKey::new(session_key.clone()).map_err(|err| runtime_error_to_js(err))?;
    core.open_node()
        .and_then(|node| node.attach_data_channel(&session, source))
        .map_err(|err| runtime_error_to_js(err))?;
    install_data_channel_event_handlers(core, rtc_control, bootstrap, session_key, channel)?;
    Ok(())
}

fn install_data_channel_event_handlers(
    core: &Rc<BrowserRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    session_key: String,
    channel: RtcDataChannel,
) -> Result<(), JsValue> {
    channel.set_binary_type(RtcDataChannelType::Arraybuffer);
    let node = core.open_node().map_err(|err| runtime_error_to_js(err))?;
    let session = node
        .session_snapshot(
            &BrowserSessionKey::new(session_key.clone()).map_err(|err| runtime_error_to_js(err))?,
        )
        .map_err(|err| runtime_error_to_js(err))?;
    let remote_addr = WebRtcAddr::session(session.remote, session.dial_id.0).to_custom_addr();
    let session_id = session.dial_id.0;
    let session_config = node.session_config();
    let open_core = core.clone();
    let open_control = rtc_control.clone();
    let open_bootstrap = bootstrap.clone();
    let open_session_key = session_key.clone();
    let open_handler = Closure::wrap(Box::new(move |_event: Event| {
        apply_data_channel_open(
            open_core.clone(),
            open_control.clone(),
            open_bootstrap.clone(),
            &open_session_key,
        );
    }) as Box<dyn FnMut(_)>);
    channel.set_onopen(Some(open_handler.as_ref().unchecked_ref()));

    let message_hub = node.session_hub();
    let message_remote_addr = remote_addr.clone();
    let message_frame_config = session_config.frame;
    let message_session_key = session_key.clone();
    let message_handler = Closure::wrap(Box::new(move |event: MessageEvent| {
        let data = event.data();
        let bytes = match js_array_buffer_like_to_vec(&data) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::rtc",
                    session_key = %message_session_key,
                    error = %js_error_message(&error),
                    "dropping non-binary RTCDataChannel message"
                );
                return;
            }
        };
        let byte_len = bytes.len();
        let frame = match WebRtcPacketFrame::decode(&bytes, message_frame_config.max_payload_len) {
            Ok(frame) => frame,
            Err(err) => {
                message_hub.record_data_channel_rx_invalid_frame();
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::rtc",
                    session_key = %message_session_key,
                    bytes = byte_len,
                    %err,
                    "dropping invalid RTCDataChannel packet frame"
                );
                return;
            }
        };
        let payload_bytes = frame.payload.len();
        message_hub.record_data_channel_rx_message(byte_len, payload_bytes);
        if frame.session_id != session_id {
            message_hub.record_data_channel_rx_wrong_session();
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::rtc",
                session_key = %message_session_key,
                expected_session_id = ?session_id,
                actual_session_id = ?frame.session_id,
                bytes = byte_len,
                "dropping RTCDataChannel datagram for wrong session"
            );
            return;
        }
        let result = message_hub.push_received(QueuedPacket {
            source: message_remote_addr.clone(),
            frame,
        });
        if let Err(err) = result {
            message_hub.record_data_channel_rx_enqueue_failure();
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::rtc",
                session_key = %message_session_key,
                bytes = byte_len,
                %err,
                "failed to enqueue RTCDataChannel packet"
            );
        } else {
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_runtime::rtc",
                session_key = %message_session_key,
                bytes = byte_len,
                "enqueued RTCDataChannel datagram"
            );
        }
    }) as Box<dyn FnMut(_)>);
    channel.set_onmessage(Some(message_handler.as_ref().unchecked_ref()));

    let error_core = core.clone();
    let error_control = rtc_control.clone();
    let error_bootstrap = bootstrap.clone();
    let error_session_key = session_key.clone();
    let error_handler = Closure::wrap(Box::new(move |_event: Event| {
        apply_data_channel_failed(
            error_core.clone(),
            error_control.clone(),
            error_bootstrap.clone(),
            &error_session_key,
            "RTCDataChannel error",
        );
    }) as Box<dyn FnMut(_)>);
    channel.set_onerror(Some(error_handler.as_ref().unchecked_ref()));

    let close_core = core.clone();
    let close_control = rtc_control.clone();
    let close_bootstrap = bootstrap.clone();
    let close_session_key = session_key.clone();
    let close_hub = node.session_hub();
    let close_session_id = session_id;
    let close_handler = Closure::wrap(Box::new(move |_event: Event| {
        close_hub.unregister_outbound_session(close_session_id);
        apply_data_channel_closed(
            close_core.clone(),
            close_control.clone(),
            close_bootstrap.clone(),
            &close_session_key,
            "RTCDataChannel closed",
        );
    }) as Box<dyn FnMut(_)>);
    channel.set_onclose(Some(close_handler.as_ref().unchecked_ref()));

    if channel.ready_state() == RtcDataChannelState::Open {
        apply_data_channel_open(
            core.clone(),
            rtc_control.clone(),
            bootstrap.clone(),
            &session_key,
        );
    }

    let outbound_rx = node.session_hub().register_outbound_session(session_id);
    spawn_outbound_data_channel_pump(
        core.clone(),
        rtc_control.clone(),
        bootstrap.clone(),
        channel.clone(),
        node.session_hub(),
        outbound_rx,
        remote_addr.clone(),
        session_key.clone(),
        session_id,
        session_config
            .data_channel
            .buffered_amount_low_threshold
            .min(u32::MAX as usize) as u32,
        session_config
            .data_channel
            .buffered_amount_high_threshold
            .min(u32::MAX as usize) as u32,
    );

    rtc_registry::retain_data_channel_handlers(
        rtc_control,
        channel,
        open_handler,
        message_handler,
        error_handler,
        close_handler,
    );
    Ok(())
}

fn apply_data_channel_open(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    session_key: &str,
) {
    apply_data_channel_result(
        core,
        rtc_control,
        bootstrap,
        session_key,
        RtcLifecycleEvent::DataChannelOpen,
    );
}

fn apply_data_channel_failed(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    session_key: &str,
    message: &str,
) {
    apply_data_channel_result(
        core,
        rtc_control,
        bootstrap,
        session_key,
        RtcLifecycleEvent::WebRtcFailed {
            message: message.to_owned(),
        },
    );
}

fn apply_data_channel_closed(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    session_key: &str,
    message: &str,
) {
    apply_data_channel_result(
        core,
        rtc_control,
        bootstrap,
        session_key,
        RtcLifecycleEvent::WebRtcClosed {
            message: message.to_owned(),
        },
    );
}

fn apply_data_channel_result(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    session_key: &str,
    event: RtcLifecycleEvent,
) {
    let Ok(session_key) = BrowserSessionKey::new(session_key.to_owned()) else {
        return;
    };
    let input = RtcLifecycleInput {
        session_key,
        event,
        error_message: None,
    };
    let _ = rtc_registry::apply_rtc_lifecycle_result(&core, &rtc_control, &bootstrap, input);
}

fn spawn_outbound_data_channel_pump(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    channel: RtcDataChannel,
    hub: SessionHub,
    outbound_rx: OutboundPacketReceiver,
    destination: CustomAddr,
    session_key: String,
    session_id: [u8; 16],
    buffered_amount_low_threshold: u32,
    buffered_amount_high_threshold: u32,
) {
    spawn_local(async move {
        pump_outbound_data_channel(
            core,
            rtc_control,
            bootstrap,
            channel,
            hub,
            outbound_rx,
            destination,
            session_key,
            session_id,
            buffered_amount_low_threshold,
            buffered_amount_high_threshold,
        )
        .await;
    });
}

async fn pump_outbound_data_channel(
    core: Rc<BrowserRuntimeCore>,
    rtc_control: Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: Rc<BrowserBootstrapRuntime>,
    channel: RtcDataChannel,
    hub: SessionHub,
    mut outbound_rx: OutboundPacketReceiver,
    destination: CustomAddr,
    session_key: String,
    session_id: [u8; 16],
    buffered_amount_low_threshold: u32,
    buffered_amount_high_threshold: u32,
) {
    channel.set_buffered_amount_low_threshold(buffered_amount_low_threshold);
    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::rtc",
        session_key = %session_key,
        destination = ?destination,
        session_id = ?session_id,
        ready_state = ?channel.ready_state(),
        buffered_amount_low_threshold,
        buffered_amount_high_threshold,
        "started RTCDataChannel outbound pump"
    );
    match wait_for_data_channel_open(&channel, &session_key).await {
        DataChannelOpenEvent::Open => {}
        DataChannelOpenEvent::Closed => {
            hub.unregister_outbound_session(session_id);
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::rtc",
                session_key = %session_key,
                destination = ?destination,
                ready_state = ?channel.ready_state(),
                "stopping RTCDataChannel outbound pump before open because channel closed"
            );
            return;
        }
        DataChannelOpenEvent::Failed => {
            hub.unregister_outbound_session(session_id);
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::rtc",
                session_key = %session_key,
                destination = ?destination,
                ready_state = ?channel.ready_state(),
                "stopping RTCDataChannel outbound pump before open because channel failed"
            );
            apply_data_channel_failed(
                core,
                rtc_control,
                bootstrap,
                &session_key,
                "RTCDataChannel failed before opening",
            );
            return;
        }
    }
    let mut packet_count = 0usize;
    loop {
        let packet = match outbound_rx.recv().await {
            Some(packet) => packet,
            None => {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::rtc",
                    session_key = %session_key,
                    destination = ?destination,
                    "stopping RTCDataChannel outbound pump because hub closed"
                );
                return;
            }
        };
        packet_count += 1;
        hub.record_data_channel_pump_pop();
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::rtc",
            session_key = %session_key,
            destination = ?packet.destination,
            packet_count,
            bytes = packet.bytes.len(),
            ready_state = ?channel.ready_state(),
            buffered_amount = channel.buffered_amount(),
            "RTCDataChannel pump popped outbound packet"
        );
        if matches!(
            channel.ready_state(),
            RtcDataChannelState::Closing | RtcDataChannelState::Closed
        ) {
            hub.unregister_outbound_session(session_id);
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::rtc",
                session_key = %session_key,
                packet_count,
                ready_state = ?channel.ready_state(),
                "stopping RTCDataChannel outbound pump because channel closed"
            );
            return;
        }
        if channel
            .buffered_amount()
            .saturating_add(packet.bytes.len() as u32)
            > buffered_amount_high_threshold
        {
            match wait_for_data_channel_capacity(
                &channel,
                buffered_amount_low_threshold,
                buffered_amount_high_threshold,
                &session_key,
                packet_count,
            )
            .await
            {
                DataChannelCapacityEvent::Low => {}
                DataChannelCapacityEvent::Closed => {
                    hub.unregister_outbound_session(session_id);
                    tracing::debug!(
                        target: "iroh_webrtc_transport::browser_runtime::rtc",
                        session_key = %session_key,
                        packet_count,
                        ready_state = ?channel.ready_state(),
                        buffered_amount = channel.buffered_amount(),
                        "stopping RTCDataChannel outbound pump while waiting for capacity because channel closed"
                    );
                    return;
                }
                DataChannelCapacityEvent::Failed => {
                    hub.unregister_outbound_session(session_id);
                    tracing::debug!(
                        target: "iroh_webrtc_transport::browser_runtime::rtc",
                        session_key = %session_key,
                        packet_count,
                        ready_state = ?channel.ready_state(),
                        buffered_amount = channel.buffered_amount(),
                        "stopping RTCDataChannel outbound pump while waiting for capacity because channel failed"
                    );
                    apply_data_channel_failed(
                        core,
                        rtc_control,
                        bootstrap,
                        &session_key,
                        "RTCDataChannel failed while waiting for send capacity",
                    );
                    return;
                }
            }
        }
        let buffered_amount_before = channel.buffered_amount();
        if channel.send_with_u8_array(&packet.bytes).is_ok() {
            hub.record_data_channel_send(
                packet.bytes.len(),
                buffered_amount_before.into(),
                channel.buffered_amount().into(),
            );
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_runtime::rtc",
                session_key = %session_key,
                packet_count,
                bytes = packet.bytes.len(),
                ready_state = ?channel.ready_state(),
                buffered_amount_before,
                buffered_amount = channel.buffered_amount(),
                "RTCDataChannel pump sent outbound packet"
            );
        } else {
            hub.record_data_channel_send_failure();
            hub.unregister_outbound_session(session_id);
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::rtc",
                session_key = %session_key,
                packet_count,
                ready_state = ?channel.ready_state(),
                buffered_amount = channel.buffered_amount(),
                "stopping RTCDataChannel outbound pump after send failure"
            );
            apply_data_channel_failed(
                core,
                rtc_control,
                bootstrap,
                &session_key,
                "RTCDataChannel send failed",
            );
            return;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataChannelOpenEvent {
    Open,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataChannelCapacityEvent {
    Low,
    Closed,
    Failed,
}

async fn wait_for_data_channel_open(
    channel: &RtcDataChannel,
    session_key: &str,
) -> DataChannelOpenEvent {
    match channel.ready_state() {
        RtcDataChannelState::Open => return DataChannelOpenEvent::Open,
        RtcDataChannelState::Closing | RtcDataChannelState::Closed => {
            return DataChannelOpenEvent::Closed;
        }
        RtcDataChannelState::Connecting => {}
        _ => {}
    }

    let (tx, rx) = oneshot::channel();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let ready_tx = tx.clone();

    let open_tx = tx.clone();
    let open_handler = Closure::wrap(Box::new(move |_event: Event| {
        if let Some(tx) = open_tx.borrow_mut().take() {
            let _ = tx.send(DataChannelOpenEvent::Open);
        }
    }) as Box<dyn FnMut(_)>);
    let close_tx = tx.clone();
    let close_handler = Closure::wrap(Box::new(move |_event: Event| {
        if let Some(tx) = close_tx.borrow_mut().take() {
            let _ = tx.send(DataChannelOpenEvent::Closed);
        }
    }) as Box<dyn FnMut(_)>);
    let error_tx = tx;
    let error_handler = Closure::wrap(Box::new(move |_event: Event| {
        if let Some(tx) = error_tx.borrow_mut().take() {
            let _ = tx.send(DataChannelOpenEvent::Failed);
        }
    }) as Box<dyn FnMut(_)>);

    let open_listener_added = channel
        .add_event_listener_with_callback("open", open_handler.as_ref().unchecked_ref())
        .is_ok();
    let close_listener_added = channel
        .add_event_listener_with_callback("close", close_handler.as_ref().unchecked_ref())
        .is_ok();
    let error_listener_added = channel
        .add_event_listener_with_callback("error", error_handler.as_ref().unchecked_ref())
        .is_ok();
    if !open_listener_added || !close_listener_added || !error_listener_added {
        if open_listener_added {
            let _ = channel
                .remove_event_listener_with_callback("open", open_handler.as_ref().unchecked_ref());
        }
        if close_listener_added {
            let _ = channel.remove_event_listener_with_callback(
                "close",
                close_handler.as_ref().unchecked_ref(),
            );
        }
        if error_listener_added {
            let _ = channel.remove_event_listener_with_callback(
                "error",
                error_handler.as_ref().unchecked_ref(),
            );
        }
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_runtime::rtc",
            session_key,
            ready_state = ?channel.ready_state(),
            "could not install RTCDataChannel open listeners"
        );
        return DataChannelOpenEvent::Open;
    }

    match channel.ready_state() {
        RtcDataChannelState::Open => {
            if let Some(tx) = ready_tx.borrow_mut().take() {
                let _ = tx.send(DataChannelOpenEvent::Open);
            }
        }
        RtcDataChannelState::Closing | RtcDataChannelState::Closed => {
            if let Some(tx) = ready_tx.borrow_mut().take() {
                let _ = tx.send(DataChannelOpenEvent::Closed);
            }
        }
        _ => {}
    }

    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::rtc",
        session_key,
        ready_state = ?channel.ready_state(),
        "waiting for RTCDataChannel to open before sending"
    );
    let result = rx.await.unwrap_or(DataChannelOpenEvent::Failed);
    let _ =
        channel.remove_event_listener_with_callback("open", open_handler.as_ref().unchecked_ref());
    let _ = channel
        .remove_event_listener_with_callback("close", close_handler.as_ref().unchecked_ref());
    let _ = channel
        .remove_event_listener_with_callback("error", error_handler.as_ref().unchecked_ref());
    result
}

async fn wait_for_data_channel_capacity(
    channel: &RtcDataChannel,
    low_threshold: u32,
    high_threshold: u32,
    session_key: &str,
    packet_count: usize,
) -> DataChannelCapacityEvent {
    if channel.buffered_amount() <= low_threshold {
        return DataChannelCapacityEvent::Low;
    }
    if matches!(
        channel.ready_state(),
        RtcDataChannelState::Closing | RtcDataChannelState::Closed
    ) {
        return DataChannelCapacityEvent::Closed;
    }

    channel.set_buffered_amount_low_threshold(low_threshold);
    let (tx, rx) = oneshot::channel();
    let tx = Rc::new(RefCell::new(Some(tx)));
    let ready_tx = tx.clone();

    let low_tx = tx.clone();
    let low_handler = Closure::wrap(Box::new(move |_event: Event| {
        if let Some(tx) = low_tx.borrow_mut().take() {
            let _ = tx.send(DataChannelCapacityEvent::Low);
        }
    }) as Box<dyn FnMut(_)>);
    let close_tx = tx.clone();
    let close_handler = Closure::wrap(Box::new(move |_event: Event| {
        if let Some(tx) = close_tx.borrow_mut().take() {
            let _ = tx.send(DataChannelCapacityEvent::Closed);
        }
    }) as Box<dyn FnMut(_)>);
    let error_tx = tx;
    let error_handler = Closure::wrap(Box::new(move |_event: Event| {
        if let Some(tx) = error_tx.borrow_mut().take() {
            let _ = tx.send(DataChannelCapacityEvent::Failed);
        }
    }) as Box<dyn FnMut(_)>);

    let low_listener_added = channel
        .add_event_listener_with_callback("bufferedamountlow", low_handler.as_ref().unchecked_ref())
        .is_ok();
    let close_listener_added = channel
        .add_event_listener_with_callback("close", close_handler.as_ref().unchecked_ref())
        .is_ok();
    let error_listener_added = channel
        .add_event_listener_with_callback("error", error_handler.as_ref().unchecked_ref())
        .is_ok();
    if !low_listener_added || !close_listener_added || !error_listener_added {
        if low_listener_added {
            let _ = channel.remove_event_listener_with_callback(
                "bufferedamountlow",
                low_handler.as_ref().unchecked_ref(),
            );
        }
        if close_listener_added {
            let _ = channel.remove_event_listener_with_callback(
                "close",
                close_handler.as_ref().unchecked_ref(),
            );
        }
        if error_listener_added {
            let _ = channel.remove_event_listener_with_callback(
                "error",
                error_handler.as_ref().unchecked_ref(),
            );
        }
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_runtime::rtc",
            session_key,
            packet_count,
            low_threshold,
            high_threshold,
            buffered_amount = channel.buffered_amount(),
            "could not install RTCDataChannel backpressure listeners"
        );
        return DataChannelCapacityEvent::Low;
    }
    if channel.buffered_amount() <= low_threshold {
        if let Some(tx) = ready_tx.borrow_mut().take() {
            let _ = tx.send(DataChannelCapacityEvent::Low);
        }
    }

    tracing::trace!(
        target: "iroh_webrtc_transport::browser_runtime::rtc",
        session_key,
        packet_count,
        low_threshold,
        high_threshold,
        buffered_amount = channel.buffered_amount(),
        "waiting for RTCDataChannel buffered amount to drain"
    );
    let result = rx.await.unwrap_or(DataChannelCapacityEvent::Failed);
    let _ = channel.remove_event_listener_with_callback(
        "bufferedamountlow",
        low_handler.as_ref().unchecked_ref(),
    );
    let _ = channel
        .remove_event_listener_with_callback("close", close_handler.as_ref().unchecked_ref());
    let _ = channel
        .remove_event_listener_with_callback("error", error_handler.as_ref().unchecked_ref());
    result
}
