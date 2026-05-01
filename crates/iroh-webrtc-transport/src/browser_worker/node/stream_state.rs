use std::sync::atomic::AtomicBool;

use iroh::endpoint::{RecvStream, SendStream};

pub(super) struct WorkerStreamState {
    pub(super) send: tokio::sync::Mutex<SendStream>,
    pub(super) recv: tokio::sync::Mutex<RecvStream>,
    pub(super) closed_send: AtomicBool,
    pub(super) closed_recv: AtomicBool,
    pub(super) send_cancel: tokio::sync::Notify,
    pub(super) recv_cancel: tokio::sync::Notify,
}

impl WorkerStreamState {
    pub(super) fn new(send: SendStream, recv: RecvStream) -> Self {
        Self {
            send: tokio::sync::Mutex::new(send),
            recv: tokio::sync::Mutex::new(recv),
            closed_send: AtomicBool::new(false),
            closed_recv: AtomicBool::new(false),
            send_cancel: tokio::sync::Notify::new(),
            recv_cancel: tokio::sync::Notify::new(),
        }
    }
}

pub(super) fn notify_stream_cancel(cancel: &tokio::sync::Notify) {
    cancel.notify_waiters();
    cancel.notify_one();
}
