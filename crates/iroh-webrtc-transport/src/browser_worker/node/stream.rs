use super::*;

impl BrowserWorkerNode {
    pub(in crate::browser_worker) async fn open_bi(
        &self,
        connection_key: WorkerConnectionKey,
    ) -> BrowserWorkerResult<WorkerStreamOpenResult> {
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_worker::stream",
            connection_key = connection_key.0,
            "opening bidirectional Iroh stream"
        );
        let connection = self.iroh_connection(connection_key)?;
        let (send, recv) = connection.open_bi().await.map_err(|err| {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::stream",
                connection_key = connection_key.0,
                %err,
                "failed to open bidirectional Iroh stream"
            );
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("failed to open bidirectional stream: {err}"),
            )
        })?;
        let stream_key = self.insert_stream(send, recv)?;
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_worker::stream",
            connection_key = connection_key.0,
            stream_key = stream_key.0,
            "opened bidirectional Iroh stream"
        );
        Ok(WorkerStreamOpenResult {
            stream_key: stream_key_string(stream_key),
        })
    }

    pub(in crate::browser_worker) async fn accept_bi(
        &self,
        connection_key: WorkerConnectionKey,
    ) -> BrowserWorkerResult<WorkerStreamAcceptResult> {
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_worker::stream",
            connection_key = connection_key.0,
            "accepting bidirectional Iroh stream"
        );
        let connection = self.iroh_connection(connection_key)?;
        let (send, recv) = connection.accept_bi().await.map_err(|err| {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::stream",
                connection_key = connection_key.0,
                %err,
                "failed to accept bidirectional Iroh stream"
            );
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                format!("failed to accept bidirectional stream: {err}"),
            )
        })?;
        let stream_key = self.insert_stream(send, recv)?;
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_worker::stream",
            connection_key = connection_key.0,
            stream_key = stream_key.0,
            "accepted bidirectional Iroh stream"
        );
        Ok(WorkerStreamAcceptResult {
            done: false,
            stream_key: Some(stream_key_string(stream_key)),
        })
    }

    pub(in crate::browser_worker) async fn send_stream_chunk(
        &self,
        stream_key: WorkerStreamKey,
        chunk: &[u8],
        end_stream: bool,
    ) -> BrowserWorkerResult<WorkerStreamSendResult> {
        self.ensure_node_open_for_stream_command()?;
        let stream = self.stream_handle(stream_key)?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::stream",
            stream_key = stream_key.0,
            chunk_bytes = chunk.len(),
            end_stream,
            "writing Iroh stream chunk"
        );
        if stream.closed_send.load(Ordering::SeqCst) {
            return Err(BrowserWorkerError::closed());
        }
        let mut send = stream.send.lock().await;
        if stream.closed_send.load(Ordering::SeqCst) {
            return Err(BrowserWorkerError::closed());
        }
        let write_result = tokio::select! {
            _ = stream.send_cancel.notified() => {
                return Err(BrowserWorkerError::closed());
            }
            result = n0_future::time::timeout(WORKER_STREAM_IO_TIMEOUT, send.write_all(chunk)) => result,
        };
        write_result
            .map_err(|_| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::stream",
                    stream_key = stream_key.0,
                    chunk_bytes = chunk.len(),
                    end_stream,
                    timeout_ms = WORKER_STREAM_IO_TIMEOUT.as_millis() as u64,
                    "timed out writing Iroh stream chunk"
                );
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    format!(
                        "timed out writing stream chunk after {}ms",
                        WORKER_STREAM_IO_TIMEOUT.as_millis()
                    ),
                )
            })?
            .map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::stream",
                    stream_key = stream_key.0,
                    chunk_bytes = chunk.len(),
                    end_stream,
                    %err,
                    "failed to write Iroh stream chunk"
                );
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    format!("failed to write stream chunk: {err}"),
                )
            })?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::stream",
            stream_key = stream_key.0,
            chunk_bytes = chunk.len(),
            end_stream,
            "wrote Iroh stream chunk"
        );
        if end_stream && !stream.closed_send.swap(true, Ordering::SeqCst) {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::stream",
                stream_key = stream_key.0,
                "finishing Iroh stream send side after chunk"
            );
            send.finish().map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::stream",
                    stream_key = stream_key.0,
                    %err,
                    "failed to finish Iroh stream send side after chunk"
                );
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    format!("failed to finish stream send side: {err}"),
                )
            })?;
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::stream",
                stream_key = stream_key.0,
                "finished Iroh stream send side after chunk"
            );
        }
        Ok(WorkerStreamSendResult {
            accepted_bytes: chunk.len(),
            buffered_bytes: 0,
            backpressure: WorkerStreamBackpressureState::Ready,
        })
    }

    pub(in crate::browser_worker) async fn receive_stream_chunk(
        &self,
        stream_key: WorkerStreamKey,
        max_bytes: Option<usize>,
    ) -> BrowserWorkerResult<WorkerStreamReceiveResult> {
        self.ensure_node_open_for_stream_command()?;
        let stream = self.stream_handle(stream_key)?;
        let limit = max_bytes
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_STREAM_RECEIVE_CHUNK_BYTES);
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_worker::stream",
            stream_key = stream_key.0,
            max_bytes = limit,
            "reading Iroh stream chunk"
        );
        if stream.closed_recv.load(Ordering::SeqCst) {
            return Ok(WorkerStreamReceiveResult {
                done: true,
                chunk: None,
            });
        }
        let mut recv = stream.recv.lock().await;
        if stream.closed_recv.load(Ordering::SeqCst) {
            return Ok(WorkerStreamReceiveResult {
                done: true,
                chunk: None,
            });
        }
        let mut chunk = vec![0u8; limit];
        let len = tokio::select! {
            _ = stream.recv_cancel.notified() => {
                return Ok(WorkerStreamReceiveResult {
                    done: true,
                    chunk: None,
                });
            }
            result = recv.read(&mut chunk) => result.map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::stream",
                    stream_key = stream_key.0,
                    max_bytes = limit,
                    %err,
                    "failed to read Iroh stream chunk"
                );
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    format!("failed to read stream chunk: {err}"),
                )
            })?,
        };
        let Some(len) = len else {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::stream",
                stream_key = stream_key.0,
                "Iroh stream receive side reached EOF"
            );
            return Ok(WorkerStreamReceiveResult {
                done: true,
                chunk: None,
            });
        };
        if len == 0 {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::stream",
                stream_key = stream_key.0,
                "Iroh stream receive side returned empty chunk"
            );
            Ok(WorkerStreamReceiveResult {
                done: true,
                chunk: None,
            })
        } else {
            chunk.truncate(len);
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_worker::stream",
                stream_key = stream_key.0,
                chunk_bytes = len,
                "read Iroh stream chunk"
            );
            Ok(WorkerStreamReceiveResult {
                done: false,
                chunk: Some(chunk),
            })
        }
    }

    pub(in crate::browser_worker) async fn close_stream_send(
        &self,
        stream_key: WorkerStreamKey,
    ) -> BrowserWorkerResult<WorkerStreamCloseSendResult> {
        self.ensure_node_open_for_stream_command()?;
        let stream = self.stream_handle(stream_key)?;
        if stream.closed_send.load(Ordering::SeqCst) {
            return Ok(WorkerStreamCloseSendResult { closed: true });
        }
        let mut send = stream.send.lock().await;
        if !stream.closed_send.swap(true, Ordering::SeqCst) {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::stream",
                stream_key = stream_key.0,
                "closing Iroh stream send side"
            );
            send.finish().map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_worker::stream",
                    stream_key = stream_key.0,
                    %err,
                    "failed to close Iroh stream send side"
                );
                BrowserWorkerError::new(
                    BrowserWorkerErrorCode::WebRtcFailed,
                    format!("failed to finish stream send side: {err}"),
                )
            })?;
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_worker::stream",
                stream_key = stream_key.0,
                "closed Iroh stream send side"
            );
        }
        Ok(WorkerStreamCloseSendResult { closed: true })
    }

    pub(in crate::browser_worker) async fn reset_stream(
        &self,
        stream_key: WorkerStreamKey,
        reason: Option<String>,
    ) -> BrowserWorkerResult<WorkerStreamResetResult> {
        self.ensure_node_open_for_stream_command()?;
        let stream = self.stream_handle(stream_key)?;
        stream.closed_send.store(true, Ordering::SeqCst);
        stream.closed_recv.store(true, Ordering::SeqCst);
        notify_stream_cancel(&stream.send_cancel);
        notify_stream_cancel(&stream.recv_cancel);
        let mut send = stream.send.lock().await;
        let reason = reason.unwrap_or_else(|| "worker stream reset".to_owned());
        let _ = send.reset(0u32.into());
        drop(send);
        if let Ok(mut recv) =
            n0_future::time::timeout(WORKER_STREAM_IO_TIMEOUT, stream.recv.lock()).await
        {
            let _ = recv.stop(0u32.into());
        }
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        inner.streams.remove(&stream_key);
        drop(reason);
        Ok(WorkerStreamResetResult { reset: true })
    }

    pub(in crate::browser_worker) fn cancel_pending_stream_command(
        &self,
        request_id: u64,
        stream_key: Option<WorkerStreamKey>,
        _reason: Option<String>,
    ) -> BrowserWorkerResult<WorkerStreamCancelPendingResult> {
        self.ensure_node_open_for_stream_command()?;
        let cancelled = stream_key
            .and_then(|stream_key| self.stream_handle(stream_key).ok())
            .map(|stream| {
                stream.closed_recv.store(true, Ordering::SeqCst);
                notify_stream_cancel(&stream.recv_cancel);
                true
            })
            .unwrap_or(false);
        Ok(WorkerStreamCancelPendingResult {
            request_id,
            cancelled,
        })
    }

    fn insert_stream(
        &self,
        send: SendStream,
        recv: RecvStream,
    ) -> BrowserWorkerResult<WorkerStreamKey> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        let stream_key = WorkerStreamKey(inner.next_stream_key);
        inner.next_stream_key = inner
            .next_stream_key
            .checked_add(1)
            .expect("worker stream id exhausted");
        inner
            .streams
            .insert(stream_key, Arc::new(WorkerStreamState::new(send, recv)));
        Ok(stream_key)
    }

    fn stream_handle(
        &self,
        stream_key: WorkerStreamKey,
    ) -> BrowserWorkerResult<Arc<WorkerStreamState>> {
        let inner = self
            .inner
            .lock()
            .expect("browser worker node mutex poisoned");
        if inner.closed {
            return Err(BrowserWorkerError::closed());
        }
        inner.streams.get(&stream_key).cloned().ok_or_else(|| {
            BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "unknown worker stream key",
            )
        })
    }

    fn ensure_node_open_for_stream_command(&self) -> BrowserWorkerResult<()> {
        if self.is_closed() {
            return Err(BrowserWorkerError::closed());
        }
        Ok(())
    }
}
