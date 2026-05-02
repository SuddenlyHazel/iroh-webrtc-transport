use std::{cell::RefCell, rc::Rc};

use wasm_bindgen::{JsValue, closure::Closure};
use web_sys::{Event, MessageEvent, RtcDataChannel};

use super::js_boundary::runtime_error_to_js;
use super::*;

pub(super) struct BrowserRtcControl {
    registry: crate::browser::BrowserRtcRegistry,
    channel_handlers: Rc<RefCell<Vec<RtcDataChannelHandlers>>>,
}

struct RtcDataChannelHandlers {
    _channel: RtcDataChannel,
    _open_handler: Closure<dyn FnMut(Event)>,
    _message_handler: Closure<dyn FnMut(MessageEvent)>,
    _error_handler: Closure<dyn FnMut(Event)>,
    _close_handler: Closure<dyn FnMut(Event)>,
}

impl BrowserRtcControl {
    pub(super) fn new(registry: crate::browser::BrowserRtcRegistry) -> Self {
        Self {
            registry,
            channel_handlers: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub(super) fn registry(&self) -> crate::browser::BrowserRtcRegistry {
        self.registry.clone()
    }
}

pub(super) fn attached_rtc_registry(
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
) -> Result<crate::browser::BrowserRtcRegistry, JsValue> {
    let control = rtc_control.borrow();
    let Some(binding) = control.as_ref() else {
        return Err(JsValue::from_str(
            "main-thread RTC registry is not attached",
        ));
    };
    Ok(binding.registry())
}

pub(super) fn retain_data_channel_handlers(
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    channel: RtcDataChannel,
    open_handler: Closure<dyn FnMut(Event)>,
    message_handler: Closure<dyn FnMut(MessageEvent)>,
    error_handler: Closure<dyn FnMut(Event)>,
    close_handler: Closure<dyn FnMut(Event)>,
) {
    let control = rtc_control.borrow();
    if let Some(binding) = control.as_ref() {
        binding
            .channel_handlers
            .borrow_mut()
            .push(RtcDataChannelHandlers {
                _channel: channel,
                _open_handler: open_handler,
                _message_handler: message_handler,
                _error_handler: error_handler,
                _close_handler: close_handler,
            });
    }
}

pub(super) fn apply_rtc_lifecycle_result(
    core: &Rc<BrowserRuntimeCore>,
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    bootstrap: &Rc<BrowserBootstrapRuntime>,
    input: RtcLifecycleInput,
) -> Result<(), JsValue> {
    let should_close_peer =
        input.error_message.is_some() || rtc_lifecycle_event_closes_peer(&input.event);
    let result = core
        .open_node()
        .and_then(|node| node.handle_rtc_lifecycle_result(input))
        .map_err(|err| runtime_error_to_js(err))?;
    if should_close_peer {
        close_peer_connection_after_terminal_result(rtc_control, &result);
    }
    let _ = super::bootstrap::send_outbound_signals(bootstrap, &result.outbound_signals);
    let _ = super::dial::complete_pending_dial_from_session(
        core.clone(),
        bootstrap.clone(),
        Some(result.session_key.as_str()),
        result.session.as_ref(),
        super::bootstrap::terminal_message(&result.outbound_signals),
    );
    super::protocol_transport::complete_pending_protocol_transport_prepare_from_result(
        core.clone(),
        bootstrap.clone(),
        Some(result.session_key.as_str()),
        result.session.as_ref(),
    )
}

#[cfg(all(target_arch = "wasm32", feature = "browser"))]
fn rtc_lifecycle_event_closes_peer(event: &RtcLifecycleEvent) -> bool {
    matches!(
        event,
        RtcLifecycleEvent::WebRtcFailed { .. } | RtcLifecycleEvent::WebRtcClosed { .. }
    )
}

#[cfg(not(all(target_arch = "wasm32", feature = "browser")))]
fn rtc_lifecycle_event_closes_peer(event: &RtcLifecycleEvent) -> bool {
    matches!(event, RtcLifecycleEvent::WebRtcFailed { .. })
}

fn close_peer_connection_after_terminal_result(
    rtc_control: &Rc<RefCell<Option<BrowserRtcControl>>>,
    result: &RtcLifecycleResult,
) {
    let registry = match attached_rtc_registry(rtc_control) {
        Ok(registry) => registry,
        Err(error) => {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser::rtc",
                error = ?error,
                session_key = %result.session_key,
                "failed to close terminal WebRTC peer connection"
            );
            return;
        }
    };
    let reason = result
        .session
        .as_ref()
        .and_then(|session| session.terminal_sent)
        .map(terminal_reason_string)
        .map(str::to_owned);
    if let Err(error) = registry.close_peer_connection(&result.session_key, reason) {
        tracing::debug!(
            target: "iroh_webrtc_transport::browser::rtc",
            error = %error,
            session_key = %result.session_key,
            "failed to close terminal WebRTC peer connection"
        );
    }
}

fn terminal_reason_string(reason: WebRtcTerminalReason) -> &'static str {
    match reason {
        WebRtcTerminalReason::WebRtcFailed => "webrtcFailed",
        WebRtcTerminalReason::FallbackSelected => "fallbackSelected",
        WebRtcTerminalReason::Cancelled => "cancelled",
        WebRtcTerminalReason::Closed => "closed",
    }
}
