use super::*;

impl BrowserRuntimeNode {
    pub(in crate::browser_runtime) async fn open_bi(
        &self,
        connection_key: BrowserConnectionKey,
    ) -> BrowserRuntimeResult<BrowserStreamInfo> {
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_runtime::stream",
            connection_key = connection_key.0,
            "opening bidirectional Iroh stream"
        );
        let connection = self.iroh_connection(connection_key)?;
        let (send, recv) = connection.open_bi().await.map_err(|err| {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::stream",
                connection_key = connection_key.0,
                %err,
                "failed to open bidirectional Iroh stream"
            );
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                format!("failed to open bidirectional stream: {err}"),
            )
        })?;
        let stream_key = self.insert_stream(send, recv)?;
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_runtime::stream",
            connection_key = connection_key.0,
            stream_key = stream_key.0,
            "opened bidirectional Iroh stream"
        );
        Ok(BrowserStreamInfo {
            stream_key: stream_key_string(stream_key),
        })
    }

    pub(in crate::browser_runtime) async fn accept_bi(
        &self,
        connection_key: BrowserConnectionKey,
    ) -> BrowserRuntimeResult<BrowserStreamAcceptInfo> {
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_runtime::stream",
            connection_key = connection_key.0,
            "accepting bidirectional Iroh stream"
        );
        let connection = self.iroh_connection(connection_key)?;
        let (send, recv) = connection.accept_bi().await.map_err(|err| {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::stream",
                connection_key = connection_key.0,
                %err,
                "failed to accept bidirectional Iroh stream"
            );
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                format!("failed to accept bidirectional stream: {err}"),
            )
        })?;
        let stream_key = self.insert_stream(send, recv)?;
        tracing::debug!(
            target: "iroh_webrtc_transport::browser_runtime::stream",
            connection_key = connection_key.0,
            stream_key = stream_key.0,
            "accepted bidirectional Iroh stream"
        );
        Ok(BrowserStreamAcceptInfo {
            done: false,
            stream_key: Some(stream_key_string(stream_key)),
        })
    }

    pub(in crate::browser_runtime) async fn send_stream_chunk(
        &self,
        stream_key: BrowserStreamKey,
        chunk: &[u8],
        end_stream: bool,
    ) -> BrowserRuntimeResult<BrowserStreamSendOutcome> {
        self.ensure_node_open_for_stream()?;
        let stream = self.stream_handle(stream_key)?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::stream",
            stream_key = stream_key.0,
            chunk_bytes = chunk.len(),
            end_stream,
            "writing Iroh stream chunk"
        );
        if stream.closed_send.load(Ordering::SeqCst) {
            return Err(BrowserRuntimeError::closed());
        }
        let mut send = stream.send.lock().await;
        if stream.closed_send.load(Ordering::SeqCst) {
            return Err(BrowserRuntimeError::closed());
        }
        let write_result = tokio::select! {
            _ = stream.send_cancel.notified() => {
                return Err(BrowserRuntimeError::closed());
            }
            result = n0_future::time::timeout(BROWSER_STREAM_IO_TIMEOUT, send.write_all(chunk)) => result,
        };
        write_result
            .map_err(|_| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::stream",
                    stream_key = stream_key.0,
                    chunk_bytes = chunk.len(),
                    end_stream,
                    timeout_ms = BROWSER_STREAM_IO_TIMEOUT.as_millis() as u64,
                    "timed out writing Iroh stream chunk"
                );
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    format!(
                        "timed out writing stream chunk after {}ms",
                        BROWSER_STREAM_IO_TIMEOUT.as_millis()
                    ),
                )
            })?
            .map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::stream",
                    stream_key = stream_key.0,
                    chunk_bytes = chunk.len(),
                    end_stream,
                    %err,
                    "failed to write Iroh stream chunk"
                );
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    format!("failed to write stream chunk: {err}"),
                )
            })?;
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::stream",
            stream_key = stream_key.0,
            chunk_bytes = chunk.len(),
            end_stream,
            "wrote Iroh stream chunk"
        );
        if end_stream && !stream.closed_send.swap(true, Ordering::SeqCst) {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::stream",
                stream_key = stream_key.0,
                "finishing Iroh stream send side after chunk"
            );
            send.finish().map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::stream",
                    stream_key = stream_key.0,
                    %err,
                    "failed to finish Iroh stream send side after chunk"
                );
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    format!("failed to finish stream send side: {err}"),
                )
            })?;
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::stream",
                stream_key = stream_key.0,
                "finished Iroh stream send side after chunk"
            );
        }
        Ok(BrowserStreamSendOutcome {
            accepted_bytes: chunk.len(),
            buffered_bytes: 0,
            backpressure: BrowserStreamBackpressureState::Ready,
        })
    }

    pub(in crate::browser_runtime) async fn receive_stream_chunk(
        &self,
        stream_key: BrowserStreamKey,
        max_bytes: Option<usize>,
    ) -> BrowserRuntimeResult<BrowserStreamReceiveOutcome> {
        self.ensure_node_open_for_stream()?;
        let stream = self.stream_handle(stream_key)?;
        let limit = max_bytes
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_STREAM_RECEIVE_CHUNK_BYTES);
        tracing::trace!(
            target: "iroh_webrtc_transport::browser_runtime::stream",
            stream_key = stream_key.0,
            max_bytes = limit,
            "reading Iroh stream chunk"
        );
        if stream.closed_recv.load(Ordering::SeqCst) {
            return Ok(BrowserStreamReceiveOutcome {
                done: true,
                chunk: None,
            });
        }
        let mut recv = stream.recv.lock().await;
        if stream.closed_recv.load(Ordering::SeqCst) {
            return Ok(BrowserStreamReceiveOutcome {
                done: true,
                chunk: None,
            });
        }
        let mut chunk = vec![0u8; limit];
        let len = tokio::select! {
            _ = stream.recv_cancel.notified() => {
                return Ok(BrowserStreamReceiveOutcome {
                    done: true,
                    chunk: None,
                });
            }
            result = recv.read(&mut chunk) => result.map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::stream",
                    stream_key = stream_key.0,
                    max_bytes = limit,
                    %err,
                    "failed to read Iroh stream chunk"
                );
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    format!("failed to read stream chunk: {err}"),
                )
            })?,
        };
        let Some(len) = len else {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::stream",
                stream_key = stream_key.0,
                "Iroh stream receive side reached EOF"
            );
            return Ok(BrowserStreamReceiveOutcome {
                done: true,
                chunk: None,
            });
        };
        if len == 0 {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::stream",
                stream_key = stream_key.0,
                "Iroh stream receive side returned empty chunk"
            );
            Ok(BrowserStreamReceiveOutcome {
                done: true,
                chunk: None,
            })
        } else {
            chunk.truncate(len);
            tracing::trace!(
                target: "iroh_webrtc_transport::browser_runtime::stream",
                stream_key = stream_key.0,
                chunk_bytes = len,
                "read Iroh stream chunk"
            );
            Ok(BrowserStreamReceiveOutcome {
                done: false,
                chunk: Some(chunk),
            })
        }
    }

    pub(in crate::browser_runtime) async fn close_stream_send(
        &self,
        stream_key: BrowserStreamKey,
    ) -> BrowserRuntimeResult<BrowserStreamCloseSendOutcome> {
        self.ensure_node_open_for_stream()?;
        let stream = self.stream_handle(stream_key)?;
        if stream.closed_send.load(Ordering::SeqCst) {
            return Ok(BrowserStreamCloseSendOutcome { closed: true });
        }
        let mut send = stream.send.lock().await;
        if !stream.closed_send.swap(true, Ordering::SeqCst) {
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::stream",
                stream_key = stream_key.0,
                "closing Iroh stream send side"
            );
            send.finish().map_err(|err| {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::stream",
                    stream_key = stream_key.0,
                    %err,
                    "failed to close Iroh stream send side"
                );
                BrowserRuntimeError::new(
                    BrowserRuntimeErrorCode::WebRtcFailed,
                    format!("failed to finish stream send side: {err}"),
                )
            })?;
            tracing::debug!(
                target: "iroh_webrtc_transport::browser_runtime::stream",
                stream_key = stream_key.0,
                "closed Iroh stream send side"
            );
        }
        Ok(BrowserStreamCloseSendOutcome { closed: true })
    }

    fn insert_stream(
        &self,
        send: SendStream,
        recv: RecvStream,
    ) -> BrowserRuntimeResult<BrowserStreamKey> {
        let mut inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        let stream_key = BrowserStreamKey(inner.next_stream_key);
        inner.next_stream_key = inner
            .next_stream_key
            .checked_add(1)
            .expect("browser stream id exhausted");
        inner
            .streams
            .insert(stream_key, Arc::new(BrowserStreamState::new(send, recv)));
        Ok(stream_key)
    }

    fn stream_handle(
        &self,
        stream_key: BrowserStreamKey,
    ) -> BrowserRuntimeResult<Arc<BrowserStreamState>> {
        let inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        inner.streams.get(&stream_key).cloned().ok_or_else(|| {
            BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
                "unknown browser stream key",
            )
        })
    }

    fn ensure_node_open_for_stream(&self) -> BrowserRuntimeResult<()> {
        if self.is_closed() {
            return Err(BrowserRuntimeError::closed());
        }
        Ok(())
    }
}
