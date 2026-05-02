use super::*;

impl BrowserWorkerRuntimeCore {
    pub(in crate::browser_worker) async fn handle_command_value(
        &self,
        command: &str,
        payload: Value,
    ) -> Result<Value, BrowserWorkerWireError> {
        let command = WorkerCommand::decode(command, payload).map_err(|err| err.wire_error())?;
        self.handle_command(command)
            .await
            .map_err(|err| err.wire_error())
    }

    #[cfg(test)]
    pub(in crate::browser_worker) async fn handle_message_value(
        &self,
        message: Value,
    ) -> BrowserWorkerResult<Option<Value>> {
        worker_response_from_message(message, |command, payload| async move {
            self.handle_command_value(&command, payload).await
        })
        .await
    }

    async fn handle_command(&self, command: WorkerCommand) -> BrowserWorkerResult<Value> {
        if command.requires_open_node() {
            self.open_node()?;
        }
        match command {
            WorkerCommand::Spawn { config } => {
                let result = self.spawn_node(config, None).await?;
                to_protocol_value(result)
            }
            WorkerCommand::Dial {
                remote_addr,
                alpn,
                transport_intent,
            } => {
                let node = self.open_node()?;
                to_protocol_value(
                    node.dial_application_connection(remote_addr, alpn, transport_intent)
                        .await?,
                )
            }
            WorkerCommand::AcceptOpen { alpn } => {
                let node = self.open_node()?;
                to_protocol_value(node.accept_open_result(alpn)?)
            }
            WorkerCommand::AcceptNext { accept_id } => {
                let node = self.open_node()?;
                let result = node.accept_next_result(accept_id).await?;
                if result.done {
                    Ok(json!({ "done": true }))
                } else {
                    to_protocol_value(result)
                }
            }
            WorkerCommand::AcceptClose { accept_id } => {
                let node = self.open_node()?;
                if !node.accept_close(accept_id)? {
                    return Err(BrowserWorkerError::new(
                        BrowserWorkerErrorCode::UnsupportedAlpn,
                        "unknown accept registration",
                    ));
                }
                Ok(json!({ "closed": true }))
            }
            WorkerCommand::BootstrapSignal { input } => {
                let node = self.open_node()?;
                to_protocol_value(node.handle_bootstrap_signal(input)?)
            }
            WorkerCommand::AttachDataChannel {
                session_key,
                source,
            } => self.handle_attach_data_channel(session_key, source),
            WorkerCommand::MainRtcResult { input } => self.handle_main_rtc_result(input),
            WorkerCommand::ConnectionClose {
                connection_key,
                reason,
            } => {
                let node = self.open_node()?;
                to_protocol_value(node.connection_close(connection_key, reason)?)
            }
            WorkerCommand::NodeClose { reason } => {
                let result = match self.node() {
                    Some(node) => node.node_close_result(reason),
                    None => WorkerCloseResult {
                        closed: true,
                        outbound_signals: Vec::new(),
                    },
                };
                to_protocol_value(result)
            }
            WorkerCommand::ProtocolCommand { alpn, command } => {
                let _ = self.open_node()?;
                self.worker_protocols.handle_command(&alpn, command).await
            }
            WorkerCommand::ProtocolNextEvent { alpn } => {
                let _ = self.open_node()?;
                self.worker_protocols.next_event(&alpn).await
            }
            WorkerCommand::StreamOpenBi { connection_key } => {
                let node = self.open_node()?;
                to_protocol_value(node.open_bi(connection_key).await?)
            }
            WorkerCommand::StreamAcceptBi { connection_key } => {
                let node = self.open_node()?;
                let result = node.accept_bi(connection_key).await?;
                if result.done {
                    Ok(json!({ "done": true }))
                } else {
                    to_protocol_value(result)
                }
            }
            WorkerCommand::StreamSendChunk {
                stream_key,
                chunk,
                end_stream,
            } => {
                let node = self.open_node()?;
                to_protocol_value(
                    node.send_stream_chunk(stream_key, &chunk, end_stream)
                        .await?,
                )
            }
            WorkerCommand::StreamReceiveChunk {
                stream_key,
                max_bytes,
            } => {
                let node = self.open_node()?;
                let result = node.receive_stream_chunk(stream_key, max_bytes).await?;
                if result.done {
                    Ok(json!({ "done": true }))
                } else {
                    to_protocol_value(result)
                }
            }
            WorkerCommand::StreamCloseSend { stream_key } => {
                let node = self.open_node()?;
                to_protocol_value(node.close_stream_send(stream_key).await?)
            }
            WorkerCommand::StreamReset { stream_key, reason } => {
                let node = self.open_node()?;
                to_protocol_value(node.reset_stream(stream_key, reason).await?)
            }
            WorkerCommand::StreamCancelPending {
                request_id,
                stream_key,
                reason,
            } => {
                let node = self.open_node()?;
                to_protocol_value(
                    node.cancel_pending_stream_command(request_id, stream_key, reason)?,
                )
            }
            WorkerCommand::BenchmarkEchoOpen { alpn } => {
                let node = self.open_node()?;
                to_protocol_value(node.open_benchmark_echo(alpn)?)
            }
            WorkerCommand::BenchmarkLatency {
                remote_addr,
                alpn,
                samples,
                warmups,
                transport_intent,
            } => {
                let node = self.open_node()?;
                let connection = node
                    .dial_application_connection(remote_addr, alpn, transport_intent)
                    .await?;
                let connection_key =
                    required_connection_key(&to_protocol_value(&connection)?, "connectionKey")?;
                to_protocol_value(
                    node.benchmark_latency_on_connection(connection_key, samples, warmups)
                        .await?,
                )
            }
            WorkerCommand::BenchmarkThroughput {
                remote_addr,
                alpn,
                bytes,
                warmup_bytes,
                transport_intent,
            } => {
                let node = self.open_node()?;
                let connection = node
                    .dial_application_connection(remote_addr, alpn, transport_intent)
                    .await?;
                let connection_key =
                    required_connection_key(&to_protocol_value(&connection)?, "connectionKey")?;
                to_protocol_value(
                    node.benchmark_throughput_on_connection(connection_key, bytes, warmup_bytes)
                        .await?,
                )
            }
        }
    }

    #[cfg(all(target_family = "wasm", target_os = "unknown"))]
    pub(in crate::browser_worker) fn handle_main_rtc_result_value(
        &self,
        payload: Value,
    ) -> BrowserWorkerResult<Value> {
        match WorkerCommand::decode(WORKER_MAIN_RTC_RESULT_COMMAND, payload)? {
            WorkerCommand::MainRtcResult { input } => self.handle_main_rtc_result(input),
            _ => unreachable!("worker main RTC result command decoded to another variant"),
        }
    }

    #[cfg(all(target_family = "wasm", target_os = "unknown"))]
    pub(in crate::browser_worker) fn handle_attach_data_channel_value(
        &self,
        payload: Value,
    ) -> BrowserWorkerResult<Value> {
        match WorkerCommand::decode(WORKER_ATTACH_DATA_CHANNEL_COMMAND, payload)? {
            WorkerCommand::AttachDataChannel {
                session_key,
                source,
            } => self.handle_attach_data_channel(session_key, source),
            _ => unreachable!("worker attach DataChannel command decoded to another variant"),
        }
    }

    fn handle_main_rtc_result(
        &self,
        input: WorkerMainRtcResultInput,
    ) -> BrowserWorkerResult<Value> {
        let node = self.open_node()?;
        to_protocol_value(node.handle_main_rtc_result(input)?)
    }

    fn handle_attach_data_channel(
        &self,
        session_key: WorkerSessionKey,
        source: WorkerDataChannelSource,
    ) -> BrowserWorkerResult<Value> {
        let node = self.open_node()?;
        to_protocol_value(node.attach_data_channel(&session_key, source)?)
    }

    pub(in crate::browser_worker) fn open_node(&self) -> BrowserWorkerResult<BrowserWorkerNode> {
        let Some(node) = self.node() else {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::Closed,
                "browser worker node has not been spawned",
            ));
        };
        if node.is_closed() {
            return Err(BrowserWorkerError::closed());
        }
        Ok(node)
    }
}
