use iroh::EndpointId;

use crate::core::{
    hub::WebRtcIceCandidate,
    signaling::{
        BootstrapTransportIntent, DialId, DialIdGenerator, WebRtcSignal, WebRtcTerminalReason,
    },
};

#[derive(Debug, Clone)]
#[cfg_attr(all(target_family = "wasm", target_os = "unknown"), allow(dead_code))]
pub struct DialCoordinator {
    local: EndpointId,
    remote: EndpointId,
    dial_id: DialId,
    generation: u64,
}

#[cfg_attr(all(target_family = "wasm", target_os = "unknown"), allow(dead_code))]
impl DialCoordinator {
    /// Builds an initiator coordinator with an explicit bootstrap transport intent.
    pub fn initiate_with_transport_intent(
        generator: &mut DialIdGenerator,
        remote: EndpointId,
        _transport_intent: BootstrapTransportIntent,
    ) -> Self {
        let session = generator.next(remote);
        Self::from_parts_with_transport_intent(
            generator.local(),
            remote,
            session.generation,
            session.dial_id,
            _transport_intent,
        )
    }

    /// Builds a responder coordinator while copying the dialer's transport intent.
    pub fn respond_with_transport_intent(
        local: EndpointId,
        remote: EndpointId,
        generation: u64,
        dial_id: DialId,
        _transport_intent: BootstrapTransportIntent,
    ) -> Self {
        Self::from_parts_with_transport_intent(
            local,
            remote,
            generation,
            dial_id,
            _transport_intent,
        )
    }

    /// Builds a coordinator from explicit low-level routing fields and transport intent.
    pub fn from_parts_with_transport_intent(
        local: EndpointId,
        remote: EndpointId,
        generation: u64,
        dial_id: DialId,
        _transport_intent: BootstrapTransportIntent,
    ) -> Self {
        Self {
            local,
            remote,
            dial_id,
            generation,
        }
    }

    pub fn dial_id(&self) -> DialId {
        self.dial_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn offer(&self, sdp: impl Into<String>) -> WebRtcSignal {
        WebRtcSignal::Offer {
            dial_id: self.dial_id,
            from: self.local,
            to: self.remote,
            generation: self.generation,
            sdp: sdp.into(),
        }
    }

    pub fn answer(&self, sdp: impl Into<String>) -> WebRtcSignal {
        WebRtcSignal::Answer {
            dial_id: self.dial_id,
            from: self.local,
            to: self.remote,
            generation: self.generation,
            sdp: sdp.into(),
        }
    }

    pub fn ice_candidate(&self, candidate: WebRtcIceCandidate) -> WebRtcSignal {
        WebRtcSignal::IceCandidate {
            dial_id: self.dial_id,
            from: self.local,
            to: self.remote,
            generation: self.generation,
            candidate: candidate.candidate,
            sdp_mid: candidate.sdp_mid,
            sdp_mline_index: candidate.sdp_mline_index,
        }
    }

    pub fn end_of_candidates(&self) -> WebRtcSignal {
        WebRtcSignal::EndOfCandidates {
            dial_id: self.dial_id,
            from: self.local,
            to: self.remote,
            generation: self.generation,
        }
    }

    #[cfg_attr(
        not(all(target_family = "wasm", target_os = "unknown")),
        allow(dead_code)
    )]
    pub fn terminal(&self, reason: WebRtcTerminalReason) -> WebRtcSignal {
        WebRtcSignal::terminal(
            self.dial_id,
            self.local,
            self.remote,
            self.generation,
            reason,
        )
    }

    #[cfg_attr(
        not(all(target_family = "wasm", target_os = "unknown")),
        allow(dead_code)
    )]
    pub fn terminal_with_message(
        &self,
        reason: WebRtcTerminalReason,
        message: impl Into<String>,
    ) -> WebRtcSignal {
        WebRtcSignal::terminal_with_message(
            self.dial_id,
            self.local,
            self.remote,
            self.generation,
            reason,
            Some(message.into()),
        )
    }

    pub fn accepts(&self, signal: &WebRtcSignal) -> bool {
        signal.dial_id() == self.dial_id
            && signal.generation() == self.generation
            && signal.is_for(self.local, self.remote)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint_id() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    #[test]
    fn coordinator_builds_routed_signals() {
        let local = endpoint_id();
        let remote = endpoint_id();
        let coordinator = DialCoordinator::from_parts_with_transport_intent(
            local,
            remote,
            3,
            DialId([9; 16]),
            BootstrapTransportIntent::WebRtcOnly,
        );

        let offer = coordinator.offer("v=0");

        assert_eq!(offer.from(), local);
        assert_eq!(offer.to(), remote);
        assert_eq!(offer.dial_id(), DialId([9; 16]));
        assert!(offer.is_for(remote, local));
        assert!(!coordinator.accepts(&offer));
    }

    #[test]
    fn coordinator_accepts_matching_remote_signal() {
        let local = endpoint_id();
        let remote = endpoint_id();
        let local_coordinator = DialCoordinator::from_parts_with_transport_intent(
            local,
            remote,
            5,
            DialId([5; 16]),
            BootstrapTransportIntent::WebRtcOnly,
        );
        let remote_coordinator = DialCoordinator::respond_with_transport_intent(
            remote,
            local,
            5,
            local_coordinator.dial_id(),
            BootstrapTransportIntent::WebRtcOnly,
        );

        assert!(local_coordinator.accepts(&remote_coordinator.answer("v=0")));
        assert!(!local_coordinator.accepts(&local_coordinator.offer("v=0")));
    }

    #[test]
    fn coordinator_rejects_matching_route_from_wrong_generation() {
        let local = endpoint_id();
        let remote = endpoint_id();
        let local_coordinator = DialCoordinator::from_parts_with_transport_intent(
            local,
            remote,
            5,
            DialId([5; 16]),
            BootstrapTransportIntent::WebRtcOnly,
        );
        let remote_coordinator = DialCoordinator::respond_with_transport_intent(
            remote,
            local,
            6,
            local_coordinator.dial_id(),
            BootstrapTransportIntent::WebRtcOnly,
        );

        assert!(!local_coordinator.accepts(&remote_coordinator.answer("v=0")));
    }

    #[test]
    fn initiator_coordinator_uses_generator_session() {
        let local = endpoint_id();
        let remote = endpoint_id();
        let mut generator = DialIdGenerator::with_epoch(local, [4; 16]);
        let expected = DialId::from_initiator_sequence(local, remote, [4; 16], 0);

        let coordinator = DialCoordinator::initiate_with_transport_intent(
            &mut generator,
            remote,
            BootstrapTransportIntent::WebRtcPreferred,
        );

        assert_eq!(coordinator.dial_id(), expected);
        assert_eq!(coordinator.generation(), 0);
    }

    #[test]
    fn coordinator_builds_terminal_signals() {
        let local = endpoint_id();
        let remote = endpoint_id();
        let coordinator = DialCoordinator::from_parts_with_transport_intent(
            local,
            remote,
            9,
            DialId([9; 16]),
            BootstrapTransportIntent::WebRtcOnly,
        );

        let failed =
            coordinator.terminal_with_message(WebRtcTerminalReason::WebRtcFailed, "ice failed");
        let fallback = coordinator
            .terminal_with_message(WebRtcTerminalReason::FallbackSelected, "relay selected");
        let cancelled =
            coordinator.terminal_with_message(WebRtcTerminalReason::Cancelled, "dial cancelled");
        let closed = coordinator.terminal(WebRtcTerminalReason::Closed);

        assert_eq!(
            failed.terminal_reason(),
            Some(WebRtcTerminalReason::WebRtcFailed)
        );
        assert_terminal_message(&failed, Some("ice failed"));
        assert_eq!(
            fallback.terminal_reason(),
            Some(WebRtcTerminalReason::FallbackSelected)
        );
        assert_terminal_message(&fallback, Some("relay selected"));
        assert_eq!(
            cancelled.terminal_reason(),
            Some(WebRtcTerminalReason::Cancelled)
        );
        assert_terminal_message(&cancelled, Some("dial cancelled"));
        assert_eq!(closed.terminal_reason(), Some(WebRtcTerminalReason::Closed));
        assert_terminal_message(&closed, None);
        assert!(failed.is_terminal());
    }

    #[test]
    fn coordinator_accepts_matching_terminal_signal() {
        let local = endpoint_id();
        let remote = endpoint_id();
        let local_coordinator = DialCoordinator::from_parts_with_transport_intent(
            local,
            remote,
            5,
            DialId([5; 16]),
            BootstrapTransportIntent::WebRtcOnly,
        );
        let remote_coordinator = DialCoordinator::respond_with_transport_intent(
            remote,
            local,
            5,
            local_coordinator.dial_id(),
            BootstrapTransportIntent::WebRtcOnly,
        );
        let wrong_dial = DialCoordinator::respond_with_transport_intent(
            remote,
            local,
            5,
            DialId([6; 16]),
            BootstrapTransportIntent::WebRtcOnly,
        );

        assert!(
            local_coordinator.accepts(
                &remote_coordinator
                    .terminal_with_message(WebRtcTerminalReason::WebRtcFailed, "ice failed")
            )
        );
        assert!(
            local_coordinator.accepts(
                &remote_coordinator.terminal_with_message(
                    WebRtcTerminalReason::FallbackSelected,
                    "relay selected"
                )
            )
        );
        assert!(
            !local_coordinator.accepts(
                &local_coordinator
                    .terminal_with_message(WebRtcTerminalReason::WebRtcFailed, "local failure")
            )
        );
        assert!(!local_coordinator.accepts(
            &wrong_dial.terminal_with_message(WebRtcTerminalReason::WebRtcFailed, "wrong dial")
        ));
    }

    #[test]
    fn simultaneous_opposite_direction_dials_keep_distinct_ids_and_intents() {
        let a = endpoint_id();
        let b = endpoint_id();
        let mut a_generator = DialIdGenerator::with_epoch(a, [7; 16]);
        let mut b_generator = DialIdGenerator::with_epoch(b, [7; 16]);

        let a_dial = DialCoordinator::initiate_with_transport_intent(
            &mut a_generator,
            b,
            BootstrapTransportIntent::WebRtcOnly,
        );
        let b_dial = DialCoordinator::initiate_with_transport_intent(
            &mut b_generator,
            a,
            BootstrapTransportIntent::WebRtcPreferred,
        );

        assert_eq!(a_dial.generation(), 0);
        assert_eq!(b_dial.generation(), 0);
        assert_ne!(a_dial.dial_id(), b_dial.dial_id());
    }

    fn assert_terminal_message(signal: &WebRtcSignal, expected: Option<&str>) {
        let WebRtcSignal::Terminal { message, .. } = signal else {
            panic!("expected terminal signal");
        };
        assert_eq!(message.as_deref(), expected);
    }
}
