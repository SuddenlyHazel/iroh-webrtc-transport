use super::*;
use super::{registry::WebRtcMainThreadRegistry, worker_bridge_wire::*};
use serde::Serialize;

pub(super) struct WebRtcMainThreadWorkerBridge {
    worker: Worker,
    registry: WebRtcMainThreadRegistry,
    pending: Rc<RefCell<HashMap<u64, PendingWorkerRequest>>>,
    tracker: Rc<RefCell<BrowserWorkerRequestTracker>>,
    _message_handler: Closure<dyn FnMut(MessageEvent)>,
    _error_handler: Closure<dyn FnMut(JsValue)>,
    _message_error_handler: Closure<dyn FnMut(JsValue)>,
}

impl WebRtcMainThreadWorkerBridge {
    pub(super) fn new(
        worker: Worker,
        registry: &WebRtcMainThreadRegistry,
    ) -> std::result::Result<WebRtcMainThreadWorkerBridge, JsValue> {
        let pending = Rc::new(RefCell::new(HashMap::new()));
        let tracker = Rc::new(RefCell::new(BrowserWorkerRequestTracker::default()));

        let message_worker = worker.clone();
        let message_pending = pending.clone();
        let message_tracker = tracker.clone();
        let message_handler = Closure::wrap(Box::new(move |event: MessageEvent| {
            handle_worker_message(
                message_worker.clone(),
                message_pending.clone(),
                message_tracker.clone(),
                event.data(),
            );
        }) as Box<dyn FnMut(_)>);

        let error_pending = pending.clone();
        let error_tracker = tracker.clone();
        let error_handler = Closure::wrap(Box::new(move |event: JsValue| {
            reject_all_pending_worker_requests(
                &error_pending,
                &error_tracker,
                worker_bridge_wire_error(
                    "spawnFailed",
                    &worker_error_message(&event, "browser WebRTC worker crashed"),
                    None,
                ),
            );
        }) as Box<dyn FnMut(_)>);

        let message_error_pending = pending.clone();
        let message_error_tracker = tracker.clone();
        let message_error_handler = Closure::wrap(Box::new(move |_event: JsValue| {
            reject_all_pending_worker_requests(
                &message_error_pending,
                &message_error_tracker,
                worker_bridge_wire_error(
                    "webrtcFailed",
                    "browser WebRTC worker sent an invalid message",
                    None,
                ),
            );
        }) as Box<dyn FnMut(_)>);

        worker.set_onmessage(Some(message_handler.as_ref().unchecked_ref()));
        worker.set_onerror(Some(error_handler.as_ref().unchecked_ref()));
        worker.set_onmessageerror(Some(message_error_handler.as_ref().unchecked_ref()));

        Ok(Self {
            worker,
            registry: registry.clone(),
            pending,
            tracker,
            _message_handler: message_handler,
            _error_handler: error_handler,
            _message_error_handler: message_error_handler,
        })
    }

    pub(super) fn attach_rtc_control_channel(&self) -> std::result::Result<(), JsValue> {
        let (port1, port2) = create_message_channel_ports()?;
        let message = serde_wasm_bindgen::to_value(&RtcControlPortBootstrapMessage {
            kind: "rtc-control-port",
            port: port2.clone(),
        })
        .map_err(|err| {
            JsValue::from_str(&format!(
                "failed to encode RTC control bootstrap message: {err}"
            ))
        })?;

        let transfer = js_sys::Array::new();
        transfer.push(port2.as_ref());
        if let Err(error) = self.registry.attach_rtc_control_port(port1.clone()) {
            port1.close();
            port2.close();
            return Err(error);
        }

        if let Err(error) = post_worker_message(&self.worker, &message, Some(&transfer)) {
            self.registry.detach_rtc_control_port();
            port1.close();
            port2.close();
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn request(
        &self,
        command: String,
        payload: JsValue,
        transfer: Option<js_sys::Array>,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(command, payload, transfer)
    }

    pub(super) fn spawn(&self, payload: JsValue) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(WORKER_SPAWN_COMMAND.into(), payload, None)
    }

    pub(super) fn dial(&self, payload: JsValue) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(WORKER_DIAL_COMMAND.into(), payload, None)
    }

    pub(super) fn accept_open(
        &self,
        payload: JsValue,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(WORKER_ACCEPT_OPEN_COMMAND.into(), payload, None)
    }

    pub(super) fn accept_next(
        &self,
        payload: JsValue,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(WORKER_ACCEPT_NEXT_COMMAND.into(), payload, None)
    }

    pub(super) fn accept_close(
        &self,
        payload: JsValue,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(WORKER_ACCEPT_CLOSE_COMMAND.into(), payload, None)
    }

    pub(super) fn connection_close(
        &self,
        payload: JsValue,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(WORKER_CONNECTION_CLOSE_COMMAND.into(), payload, None)
    }

    pub(super) fn node_close(
        &self,
        payload: JsValue,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(WORKER_NODE_CLOSE_COMMAND.into(), payload, None)
    }

    pub(super) fn open_bi(
        &self,
        payload: JsValue,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(STREAM_OPEN_BI_COMMAND.into(), payload, None)
    }

    pub(super) fn accept_bi(
        &self,
        payload: JsValue,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(STREAM_ACCEPT_BI_COMMAND.into(), payload, None)
    }

    pub(super) fn receive(
        &self,
        payload: JsValue,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(STREAM_RECEIVE_CHUNK_COMMAND.into(), payload, None)
    }

    pub(super) fn close_send(
        &self,
        payload: JsValue,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        self.request_worker(STREAM_CLOSE_SEND_COMMAND.into(), payload, None)
    }

    pub(super) fn close(&self) {
        self.worker.set_onmessage(None);
        self.worker.set_onerror(None);
        self.worker.set_onmessageerror(None);
        self.registry.detach_rtc_control_port();
        reject_all_pending_worker_requests(
            &self.pending,
            &self.tracker,
            worker_bridge_wire_error("closed", "WebRTC Iroh browser node shut down", None),
        );
        self.worker.terminate();
    }

    fn request_worker(
        &self,
        command: String,
        payload: JsValue,
        transfer: Option<js_sys::Array>,
    ) -> std::result::Result<js_sys::Promise, JsValue> {
        let payload = worker_request_payload(payload, &command)?;
        let id = self
            .tracker
            .borrow_mut()
            .allocate()
            .map_err(|error| worker_bridge_error_from_error(Some(&command), error))?;
        let message = worker_request_message(id, &command, payload.clone())?;
        let transfer = match transfer {
            Some(transfer) => Some(transfer),
            None => transfer_list_for_worker_request(&command, &payload)?,
        };

        let pending_slot = Rc::new(RefCell::new(None));
        let pending_capture = pending_slot.clone();
        let promise = js_sys::Promise::new(&mut move |resolve, reject| {
            *pending_capture.borrow_mut() = Some(PendingWorkerRequest { resolve, reject });
        });
        let pending_request = pending_slot.borrow_mut().take().ok_or_else(|| {
            worker_bridge_wire_error("webrtcFailed", "Promise resolver missing", None)
        })?;
        self.pending.borrow_mut().insert(id, pending_request);

        if let Err(error) = post_worker_message(&self.worker, &message, transfer.as_ref()) {
            {
                self.tracker.borrow_mut().complete(id);
            }
            let pending = {
                let mut pending = self.pending.borrow_mut();
                pending.remove(&id)
            };
            if let Some(pending) = pending {
                let _ = pending.reject.call1(&JsValue::UNDEFINED, &error);
            }
            return Err(error);
        }

        let _ = js_sys::Reflect::set(
            promise.as_ref(),
            &JsValue::from_str("requestId"),
            &JsValue::from_f64(id as f64),
        );
        Ok(promise)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RtcControlPortBootstrapMessage {
    kind: &'static str,
    #[serde(with = "serde_wasm_bindgen::preserve")]
    port: MessagePort,
}
