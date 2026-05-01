use super::*;

#[derive(Debug, Clone)]
pub(super) struct WorkerSessionState {
    pub(super) session_key: WorkerSessionKey,
    pub(super) dial_id: DialId,
    pub(super) generation: u64,
    pub(super) local: EndpointId,
    pub(super) remote: EndpointId,
    pub(super) alpn: String,
    pub(super) role: WorkerSessionRole,
    pub(super) transport_intent: BootstrapTransportIntent,
    pub(super) resolved_transport: Option<WorkerResolvedTransport>,
    pub(super) channel_attachment: WorkerDataChannelAttachmentState,
    pub(super) lifecycle: WorkerSessionLifecycle,
    pub(super) terminal_sent: Option<WebRtcTerminalReason>,
    pub(super) terminal_received: Option<WebRtcTerminalReason>,
}

impl WorkerSessionState {
    pub(super) fn new(
        session_key: WorkerSessionKey,
        dial_id: DialId,
        generation: u64,
        local: EndpointId,
        remote: EndpointId,
        alpn: String,
        role: WorkerSessionRole,
        transport_intent: BootstrapTransportIntent,
    ) -> Self {
        let (channel_attachment, lifecycle) = if transport_intent.uses_webrtc() {
            (
                WorkerDataChannelAttachmentState::AwaitingTransfer,
                WorkerSessionLifecycle::WebRtcNegotiating,
            )
        } else {
            (
                WorkerDataChannelAttachmentState::NotRequired,
                WorkerSessionLifecycle::RelaySelected,
            )
        };
        Self {
            session_key,
            dial_id,
            generation,
            local,
            remote,
            alpn,
            role,
            transport_intent,
            resolved_transport: None,
            channel_attachment,
            lifecycle,
            terminal_sent: None,
            terminal_received: None,
        }
    }

    pub(super) fn attach_transferred_channel(&mut self) -> BrowserWorkerResult<()> {
        if !matches!(
            self.channel_attachment,
            WorkerDataChannelAttachmentState::AwaitingTransfer
        ) {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::DataChannelFailed,
                "session is not waiting for transferred DataChannel",
            ));
        }
        self.channel_attachment = WorkerDataChannelAttachmentState::Transferred;
        Ok(())
    }

    pub(super) fn mark_transferred_channel_open(&mut self) -> BrowserWorkerResult<()> {
        if !matches!(
            self.channel_attachment,
            WorkerDataChannelAttachmentState::Transferred
        ) {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::DataChannelFailed,
                "transferred DataChannel has not been attached",
            ));
        }
        self.channel_attachment = WorkerDataChannelAttachmentState::Open;
        Ok(())
    }

    pub(super) fn fail(&mut self) {
        self.channel_attachment = match self.channel_attachment {
            WorkerDataChannelAttachmentState::NotRequired => {
                WorkerDataChannelAttachmentState::NotRequired
            }
            _ => WorkerDataChannelAttachmentState::Failed,
        };
        self.lifecycle = WorkerSessionLifecycle::Failed;
    }

    pub(super) fn close(&mut self) {
        self.channel_attachment = match self.channel_attachment {
            WorkerDataChannelAttachmentState::NotRequired => {
                WorkerDataChannelAttachmentState::NotRequired
            }
            _ => WorkerDataChannelAttachmentState::Closed,
        };
        self.lifecycle = WorkerSessionLifecycle::Closed;
    }

    pub(super) fn resolve_transport(
        &mut self,
        resolved_transport: WorkerResolvedTransport,
    ) -> BrowserWorkerResult<()> {
        validate_transport_resolution(self, resolved_transport)?;
        self.resolved_transport = Some(resolved_transport);
        self.lifecycle = WorkerSessionLifecycle::ApplicationReady;
        Ok(())
    }

    pub(super) fn complete_dial(
        &mut self,
        resolved_transport: WorkerResolvedTransport,
    ) -> BrowserWorkerResult<()> {
        if self.role != WorkerSessionRole::Dialer {
            return Err(BrowserWorkerError::new(
                BrowserWorkerErrorCode::WebRtcFailed,
                "session is not a dialer session",
            ));
        }
        self.resolve_transport(resolved_transport)
    }

    pub(super) fn record_terminal_received(&mut self, reason: WebRtcTerminalReason) {
        self.terminal_received = Some(reason);
        self.apply_terminal_reason(reason);
    }

    pub(super) fn record_terminal_sent(
        &mut self,
        reason: WebRtcTerminalReason,
    ) -> Option<WorkerResolvedTransport> {
        self.terminal_sent = Some(reason);
        self.apply_terminal_reason(reason)
    }

    pub(super) fn close_for_node(&mut self, message: Option<String>) -> Option<WebRtcSignal> {
        let signal = if self.terminal_sent.is_none() && self.transport_intent.uses_webrtc() {
            Some(session_terminal_signal(
                self,
                WebRtcTerminalReason::Closed,
                message,
            ))
        } else {
            None
        };
        if signal.is_some() {
            self.terminal_sent = Some(WebRtcTerminalReason::Closed);
        }
        self.close();
        signal
    }

    pub(super) fn snapshot(&self) -> WorkerSessionSnapshot {
        WorkerSessionSnapshot {
            session_key: self.session_key.clone(),
            dial_id: self.dial_id,
            generation: self.generation,
            local: self.local,
            remote: self.remote,
            alpn: self.alpn.clone(),
            role: self.role,
            transport_intent: self.transport_intent,
            resolved_transport: self.resolved_transport,
            channel_attachment: self.channel_attachment,
            lifecycle: self.lifecycle,
            terminal_sent: self.terminal_sent,
            terminal_received: self.terminal_received,
        }
    }

    fn apply_terminal_reason(
        &mut self,
        reason: WebRtcTerminalReason,
    ) -> Option<WorkerResolvedTransport> {
        match reason {
            WebRtcTerminalReason::FallbackSelected => {
                if self.transport_intent.allows_iroh_relay_fallback() {
                    self.resolved_transport = Some(WorkerResolvedTransport::IrohRelay);
                    self.lifecycle = WorkerSessionLifecycle::RelaySelected;
                    self.channel_attachment = WorkerDataChannelAttachmentState::Failed;
                    Some(WorkerResolvedTransport::IrohRelay)
                } else {
                    self.fail();
                    None
                }
            }
            WebRtcTerminalReason::WebRtcFailed => {
                self.fail();
                None
            }
            WebRtcTerminalReason::Cancelled | WebRtcTerminalReason::Closed => {
                self.close();
                None
            }
        }
    }
}
