use super::*;

#[derive(Debug, Clone)]
pub(super) struct BrowserSessionState {
    pub(super) session_key: BrowserSessionKey,
    pub(super) dial_id: DialId,
    pub(super) generation: u64,
    pub(super) local: EndpointId,
    pub(super) remote: EndpointId,
    pub(super) alpn: String,
    pub(super) role: BrowserSessionRole,
    pub(super) transport_intent: BootstrapTransportIntent,
    pub(super) resolved_transport: Option<BrowserResolvedTransport>,
    pub(super) channel_attachment: DataChannelAttachmentState,
    pub(super) lifecycle: BrowserSessionLifecycle,
    pub(super) terminal_sent: Option<WebRtcTerminalReason>,
    pub(super) terminal_received: Option<WebRtcTerminalReason>,
}

impl BrowserSessionState {
    pub(super) fn new(
        session_key: BrowserSessionKey,
        dial_id: DialId,
        generation: u64,
        local: EndpointId,
        remote: EndpointId,
        alpn: String,
        role: BrowserSessionRole,
        transport_intent: BootstrapTransportIntent,
    ) -> Self {
        let (channel_attachment, lifecycle) = if transport_intent.uses_webrtc() {
            (
                DataChannelAttachmentState::AwaitingTransfer,
                BrowserSessionLifecycle::WebRtcNegotiating,
            )
        } else {
            (
                DataChannelAttachmentState::NotRequired,
                BrowserSessionLifecycle::RelaySelected,
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

    pub(super) fn attach_transferred_channel(&mut self) -> BrowserRuntimeResult<()> {
        if !matches!(
            self.channel_attachment,
            DataChannelAttachmentState::AwaitingTransfer
        ) {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::DataChannelFailed,
                "session is not waiting for transferred DataChannel",
            ));
        }
        self.channel_attachment = DataChannelAttachmentState::Transferred;
        Ok(())
    }

    pub(super) fn mark_transferred_channel_open(&mut self) -> BrowserRuntimeResult<()> {
        if !matches!(
            self.channel_attachment,
            DataChannelAttachmentState::Transferred
        ) {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::DataChannelFailed,
                "transferred DataChannel has not been attached",
            ));
        }
        self.channel_attachment = DataChannelAttachmentState::Open;
        Ok(())
    }

    pub(super) fn fail(&mut self) {
        self.channel_attachment = match self.channel_attachment {
            DataChannelAttachmentState::NotRequired => DataChannelAttachmentState::NotRequired,
            _ => DataChannelAttachmentState::Failed,
        };
        self.lifecycle = BrowserSessionLifecycle::Failed;
    }

    pub(super) fn close(&mut self) {
        self.channel_attachment = match self.channel_attachment {
            DataChannelAttachmentState::NotRequired => DataChannelAttachmentState::NotRequired,
            _ => DataChannelAttachmentState::Closed,
        };
        self.lifecycle = BrowserSessionLifecycle::Closed;
    }

    pub(super) fn resolve_transport(
        &mut self,
        resolved_transport: BrowserResolvedTransport,
    ) -> BrowserRuntimeResult<()> {
        validate_transport_resolution(self, resolved_transport)?;
        self.resolved_transport = Some(resolved_transport);
        self.lifecycle = BrowserSessionLifecycle::ApplicationReady;
        Ok(())
    }

    pub(super) fn complete_dial(
        &mut self,
        resolved_transport: BrowserResolvedTransport,
    ) -> BrowserRuntimeResult<()> {
        if self.role != BrowserSessionRole::Dialer {
            return Err(BrowserRuntimeError::new(
                BrowserRuntimeErrorCode::WebRtcFailed,
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
    ) -> Option<BrowserResolvedTransport> {
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

    pub(super) fn snapshot(&self) -> BrowserSessionSnapshot {
        BrowserSessionSnapshot {
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
    ) -> Option<BrowserResolvedTransport> {
        match reason {
            WebRtcTerminalReason::FallbackSelected => {
                if self.transport_intent.allows_iroh_relay_fallback() {
                    self.resolved_transport = Some(BrowserResolvedTransport::IrohRelay);
                    self.lifecycle = BrowserSessionLifecycle::RelaySelected;
                    self.channel_attachment = DataChannelAttachmentState::Failed;
                    Some(BrowserResolvedTransport::IrohRelay)
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
