use std::collections::VecDeque;

use tokio::sync::oneshot;

use crate::browser_worker::{
    BrowserWorkerError, BrowserWorkerErrorCode, BrowserWorkerResult, WorkerAcceptId,
    WorkerAcceptNext, WorkerAcceptedConnection,
};

pub(super) struct AcceptRegistrationState {
    pub(super) id: WorkerAcceptId,
    pub(super) alpn: String,
    pub(super) claimed: bool,
    pub(super) queue: VecDeque<WorkerAcceptedConnection>,
    pub(super) waiters: VecDeque<oneshot::Sender<WorkerAcceptNext>>,
    capacity: usize,
}

impl AcceptRegistrationState {
    pub(super) fn new(id: WorkerAcceptId, alpn: String, capacity: usize) -> Self {
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
        connection: WorkerAcceptedConnection,
    ) -> BrowserWorkerResult<()> {
        while let Some(waiter) = self.waiters.pop_front() {
            match waiter.send(WorkerAcceptNext::Ready(connection.clone())) {
                Ok(()) => return Ok(()),
                Err(WorkerAcceptNext::Ready(_)) => continue,
                Err(WorkerAcceptNext::Done) => unreachable!("sent ready connection"),
            }
        }

        if self.queue.len() >= self.capacity {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
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
            let _ = waiter.send(WorkerAcceptNext::Done);
        }
    }

    pub(super) fn complete_waiters_done(mut self) {
        self.close_acceptor();
    }
}
