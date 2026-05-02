use std::collections::VecDeque;

use tokio::sync::oneshot;

use crate::browser_runtime::{
    BrowserAcceptId, BrowserAcceptNext, BrowserAcceptedConnection, BrowserRuntimeError,
    BrowserRuntimeErrorCode, BrowserRuntimeResult,
};

pub(super) struct AcceptRegistrationState {
    pub(super) id: BrowserAcceptId,
    pub(super) alpn: String,
    pub(super) claimed: bool,
    pub(super) queue: VecDeque<BrowserAcceptedConnection>,
    pub(super) waiters: VecDeque<oneshot::Sender<BrowserAcceptNext>>,
    capacity: usize,
}

impl AcceptRegistrationState {
    pub(super) fn new(id: BrowserAcceptId, alpn: String, capacity: usize) -> Self {
        Self {
            id,
            alpn,
            claimed: false,
            queue: VecDeque::new(),
            waiters: VecDeque::new(),
            capacity,
        }
    }

    pub(super) fn push_or_wake(
        &mut self,
        connection: BrowserAcceptedConnection,
    ) -> BrowserRuntimeResult<()> {
        while let Some(waiter) = self.waiters.pop_front() {
            match waiter.send(BrowserAcceptNext::Ready(connection.clone())) {
                Ok(()) => return Ok(()),
                Err(BrowserAcceptNext::Ready(_)) => continue,
                Err(BrowserAcceptNext::Done) => unreachable!("sent ready connection"),
            }
        }

        if self.queue.len() >= self.capacity {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                format!("accept queue is full for ALPN {:?}", self.alpn),
            ));
        }
        self.queue.push_back(connection);
        Ok(())
    }

    pub(super) fn close_acceptor(&mut self) {
        self.claimed = false;
        self.queue.clear();
        for waiter in self.waiters.drain(..) {
            let _ = waiter.send(BrowserAcceptNext::Done);
        }
    }

    pub(super) fn complete_waiters_done(mut self) {
        self.close_acceptor();
    }
}
