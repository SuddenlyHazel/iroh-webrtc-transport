//! Signaling message types used to bootstrap WebRTC sessions over Iroh streams.

use std::collections::HashMap;

use iroh::EndpointId;
use serde::{Deserialize, Serialize};

/// RTCDataChannel label used for Iroh/noq packet frames.
pub const WEBRTC_DATA_CHANNEL_LABEL: &str = "iroh-noq";
const DIAL_ID_DERIVATION_DOMAIN: &[u8] = b"iroh-webrtc-dial-id-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum BootstrapTransportIntent {
    #[serde(rename = "irohRelay")]
    IrohRelay,
    #[default]
    #[serde(rename = "webrtcPreferred")]
    WebRtcPreferred,
    #[serde(rename = "webrtcOnly")]
    WebRtcOnly,
}

impl BootstrapTransportIntent {
    pub fn uses_webrtc(self) -> bool {
        !matches!(self, Self::IrohRelay)
    }

    pub fn allows_iroh_relay_fallback(self) -> bool {
        matches!(self, Self::WebRtcPreferred)
    }

    pub fn requires_webrtc(self) -> bool {
        matches!(self, Self::WebRtcOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WebRtcTerminalReason {
    #[serde(rename = "webrtcFailed")]
    WebRtcFailed,
    #[serde(rename = "fallbackSelected")]
    FallbackSelected,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "closed")]
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DialId(pub [u8; 16]);

/// A newly allocated WebRTC bootstrap session identity.
///
/// `generation` is the initiator-owned sequence number used to derive
/// `dial_id`. The responder should copy both values from the initiator's
/// `DialRequest` rather than allocating its own sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialSessionId {
    pub dial_id: DialId,
    pub generation: u64,
}

/// Allocates deterministic `DialId`s from a random local epoch and per-remote counters.
///
/// Each endpoint owns the sequence stream for dials it initiates. The random
/// epoch prevents sequence reuse after a process restart from recreating older
/// session ids.
#[derive(Debug, Clone)]
pub struct DialIdGenerator {
    local: EndpointId,
    epoch: [u8; 16],
    next_by_remote: HashMap<EndpointId, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WebRtcSignal {
    #[serde(rename = "session")]
    DialRequest {
        dial_id: DialId,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        alpn: String,
        #[serde(rename = "transportIntent")]
        transport_intent: BootstrapTransportIntent,
    },
    Offer {
        dial_id: DialId,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        sdp: String,
    },
    Answer {
        dial_id: DialId,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        sdp: String,
    },
    IceCandidate {
        dial_id: DialId,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
    EndOfCandidates {
        dial_id: DialId,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
    },
    Terminal {
        dial_id: DialId,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        reason: WebRtcTerminalReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

impl DialId {
    /// Derives the 16-byte session id for an initiator-owned dial sequence.
    ///
    /// `initiator` and `responder` are ordered by protocol role, not sorted.
    /// Keeping the order explicit gives each side of a peer pair its own
    /// independent sequence stream.
    pub fn from_initiator_sequence(
        initiator: EndpointId,
        responder: EndpointId,
        epoch: [u8; 16],
        generation: u64,
    ) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(DIAL_ID_DERIVATION_DOMAIN);
        hasher.update(initiator.as_bytes());
        hasher.update(responder.as_bytes());
        hasher.update(&epoch);
        hasher.update(&generation.to_be_bytes());

        let hash = hasher.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&hash.as_bytes()[..16]);
        Self(bytes)
    }
}

impl DialIdGenerator {
    /// Creates a generator with a fresh random epoch for this process.
    pub fn new(local: EndpointId) -> Self {
        Self::with_epoch(local, random_epoch())
    }

    /// Creates a generator with an explicit epoch.
    ///
    /// This is primarily useful for deterministic tests and replay tooling.
    pub fn with_epoch(local: EndpointId, epoch: [u8; 16]) -> Self {
        Self {
            local,
            epoch,
            next_by_remote: HashMap::new(),
        }
    }

    #[cfg_attr(all(target_family = "wasm", target_os = "unknown"), allow(dead_code))]
    pub fn local(&self) -> EndpointId {
        self.local
    }

    /// Allocates the next initiator-owned session id for `remote`.
    pub fn next(&mut self, remote: EndpointId) -> DialSessionId {
        let generation = self.next_generation(remote);
        DialSessionId {
            dial_id: DialId::from_initiator_sequence(self.local, remote, self.epoch, generation),
            generation,
        }
    }

    fn next_generation(&mut self, remote: EndpointId) -> u64 {
        let generation = self.next_by_remote.entry(remote).or_insert(0);
        let current = *generation;
        *generation = generation
            .checked_add(1)
            .expect("dial id generation exhausted for remote endpoint");
        current
    }
}

fn random_epoch() -> [u8; 16] {
    rand::random()
}

impl From<DialSessionId> for DialId {
    fn from(session: DialSessionId) -> Self {
        session.dial_id
    }
}

impl WebRtcSignal {
    pub fn dial_request_with_alpn(
        dial_id: DialId,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        alpn: impl Into<String>,
        transport_intent: BootstrapTransportIntent,
    ) -> Self {
        Self::DialRequest {
            dial_id,
            from,
            to,
            generation,
            alpn: alpn.into(),
            transport_intent,
        }
    }

    #[cfg_attr(
        not(all(target_family = "wasm", target_os = "unknown")),
        allow(dead_code)
    )]
    pub fn terminal(
        dial_id: DialId,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        reason: WebRtcTerminalReason,
    ) -> Self {
        Self::terminal_with_message(dial_id, from, to, generation, reason, None)
    }

    #[cfg_attr(
        not(all(target_family = "wasm", target_os = "unknown")),
        allow(dead_code)
    )]
    pub fn terminal_with_message(
        dial_id: DialId,
        from: EndpointId,
        to: EndpointId,
        generation: u64,
        reason: WebRtcTerminalReason,
        message: Option<String>,
    ) -> Self {
        Self::Terminal {
            dial_id,
            from,
            to,
            generation,
            reason,
            message,
        }
    }

    pub fn dial_id(&self) -> DialId {
        match self {
            Self::DialRequest { dial_id, .. }
            | Self::Offer { dial_id, .. }
            | Self::Answer { dial_id, .. }
            | Self::IceCandidate { dial_id, .. }
            | Self::EndOfCandidates { dial_id, .. }
            | Self::Terminal { dial_id, .. } => *dial_id,
        }
    }

    pub fn from(&self) -> EndpointId {
        match self {
            Self::DialRequest { from, .. }
            | Self::Offer { from, .. }
            | Self::Answer { from, .. }
            | Self::IceCandidate { from, .. }
            | Self::EndOfCandidates { from, .. }
            | Self::Terminal { from, .. } => *from,
        }
    }

    pub fn to(&self) -> EndpointId {
        match self {
            Self::DialRequest { to, .. }
            | Self::Offer { to, .. }
            | Self::Answer { to, .. }
            | Self::IceCandidate { to, .. }
            | Self::EndOfCandidates { to, .. }
            | Self::Terminal { to, .. } => *to,
        }
    }

    pub fn generation(&self) -> u64 {
        match self {
            Self::DialRequest { generation, .. }
            | Self::Offer { generation, .. }
            | Self::Answer { generation, .. }
            | Self::IceCandidate { generation, .. }
            | Self::EndOfCandidates { generation, .. }
            | Self::Terminal { generation, .. } => *generation,
        }
    }

    #[cfg_attr(
        not(all(target_family = "wasm", target_os = "unknown")),
        allow(dead_code)
    )]
    pub fn terminal_reason(&self) -> Option<WebRtcTerminalReason> {
        match self {
            Self::Terminal { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    #[cfg_attr(
        not(all(target_family = "wasm", target_os = "unknown")),
        allow(dead_code)
    )]
    pub fn is_terminal(&self) -> bool {
        self.terminal_reason().is_some()
    }

    #[cfg_attr(all(target_family = "wasm", target_os = "unknown"), allow(dead_code))]
    pub fn is_for(&self, local: EndpointId, remote: EndpointId) -> bool {
        self.to() == local && self.from() == remote
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint_id() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    #[test]
    fn transport_intent_serializes_canonical_strings() {
        assert_eq!(
            serde_json::to_value(BootstrapTransportIntent::IrohRelay).unwrap(),
            serde_json::json!("irohRelay")
        );
        assert_eq!(
            serde_json::to_value(BootstrapTransportIntent::WebRtcPreferred).unwrap(),
            serde_json::json!("webrtcPreferred")
        );
        assert_eq!(
            serde_json::to_value(BootstrapTransportIntent::WebRtcOnly).unwrap(),
            serde_json::json!("webrtcOnly")
        );
    }

    #[test]
    fn terminal_reason_serializes_canonical_strings() {
        assert_eq!(
            serde_json::to_value(WebRtcTerminalReason::WebRtcFailed).unwrap(),
            serde_json::json!("webrtcFailed")
        );
        assert_eq!(
            serde_json::to_value(WebRtcTerminalReason::FallbackSelected).unwrap(),
            serde_json::json!("fallbackSelected")
        );
        assert_eq!(
            serde_json::to_value(WebRtcTerminalReason::Cancelled).unwrap(),
            serde_json::json!("cancelled")
        );
        assert_eq!(
            serde_json::to_value(WebRtcTerminalReason::Closed).unwrap(),
            serde_json::json!("closed")
        );
    }

    #[test]
    fn signal_round_trips_json() {
        let signal = WebRtcSignal::IceCandidate {
            dial_id: DialId([3; 16]),
            from: endpoint_id(),
            to: endpoint_id(),
            generation: 9,
            candidate: "candidate:1 1 UDP 1 127.0.0.1 9 typ host".into(),
            sdp_mid: Some("0".into()),
            sdp_mline_index: Some(0),
        };

        let encoded = serde_json::to_string(&signal).unwrap();
        let decoded: WebRtcSignal = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, signal);
    }

    #[test]
    fn signal_helpers_expose_route_metadata() {
        let from = endpoint_id();
        let to = endpoint_id();
        let dial_id = DialId::from_initiator_sequence(from, to, [1; 16], 7);
        let signal = WebRtcSignal::dial_request_with_alpn(
            dial_id,
            from,
            to,
            7,
            "",
            BootstrapTransportIntent::WebRtcOnly,
        );

        assert_eq!(signal.dial_id(), dial_id);
        assert_eq!(signal.from(), from);
        assert_eq!(signal.to(), to);
        assert_eq!(signal.generation(), 7);
        assert!(signal.is_for(to, from));
        assert!(!signal.is_for(from, to));
    }

    #[test]
    fn dial_request_serializes_transport_intent() {
        let signal = WebRtcSignal::dial_request_with_alpn(
            DialId([4; 16]),
            endpoint_id(),
            endpoint_id(),
            11,
            "",
            BootstrapTransportIntent::IrohRelay,
        );

        let value = serde_json::to_value(&signal).unwrap();

        assert_eq!(value["type"], "session");
        assert_eq!(value["transportIntent"], "irohRelay");
    }

    #[test]
    fn dial_request_without_transport_intent_is_rejected() {
        let signal = WebRtcSignal::dial_request_with_alpn(
            DialId([4; 16]),
            endpoint_id(),
            endpoint_id(),
            11,
            "",
            BootstrapTransportIntent::IrohRelay,
        );
        let mut value = serde_json::to_value(&signal).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("transportIntent")
            .unwrap();

        assert!(serde_json::from_value::<WebRtcSignal>(value).is_err());
    }

    #[test]
    fn terminal_signal_serializes_canonical_shape() {
        let from = endpoint_id();
        let to = endpoint_id();
        let signal = WebRtcSignal::terminal_with_message(
            DialId([5; 16]),
            from,
            to,
            12,
            WebRtcTerminalReason::FallbackSelected,
            Some("relay selected".into()),
        );

        let value = serde_json::to_value(&signal).unwrap();
        let decoded: WebRtcSignal = serde_json::from_value(value.clone()).unwrap();

        assert_eq!(value["type"], "terminal");
        assert_eq!(value["reason"], "fallbackSelected");
        assert_eq!(value["message"], "relay selected");
        assert_eq!(decoded, signal);
        assert_eq!(
            signal.terminal_reason(),
            Some(WebRtcTerminalReason::FallbackSelected)
        );
        assert_terminal_message(&signal, Some("relay selected"));
        assert!(signal.is_terminal());
        assert!(signal.is_for(to, from));
    }

    #[test]
    fn terminal_signal_omits_missing_message() {
        let signal = WebRtcSignal::terminal(
            DialId([5; 16]),
            endpoint_id(),
            endpoint_id(),
            12,
            WebRtcTerminalReason::Closed,
        );

        let value = serde_json::to_value(&signal).unwrap();

        assert_eq!(value["type"], "terminal");
        assert_eq!(value["reason"], "closed");
        assert!(value.get("message").is_none());
        assert_terminal_message(&signal, None);
    }

    fn assert_terminal_message(signal: &WebRtcSignal, expected: Option<&str>) {
        let WebRtcSignal::Terminal { message, .. } = signal else {
            panic!("expected terminal signal");
        };
        assert_eq!(message.as_deref(), expected);
    }

    #[test]
    fn dial_id_generator_allocates_monotonic_ids_per_remote() {
        let local = endpoint_id();
        let remote = endpoint_id();
        let mut generator = DialIdGenerator::with_epoch(local, [7; 16]);

        let first = generator.next(remote);
        let second = generator.next(remote);

        assert_eq!(first.generation, 0);
        assert_eq!(second.generation, 1);
        assert_ne!(first.dial_id, second.dial_id);
    }

    #[test]
    fn dial_id_generator_is_deterministic_for_epoch_and_sequence() {
        let local = endpoint_id();
        let remote = endpoint_id();
        let mut first_generator = DialIdGenerator::with_epoch(local, [7; 16]);
        let mut second_generator = DialIdGenerator::with_epoch(local, [7; 16]);

        assert_eq!(first_generator.next(remote), second_generator.next(remote));
    }

    #[test]
    fn dial_id_generator_namespaces_opposite_directions() {
        let a = endpoint_id();
        let b = endpoint_id();
        let mut a_generator = DialIdGenerator::with_epoch(a, [9; 16]);
        let mut b_generator = DialIdGenerator::with_epoch(b, [9; 16]);

        assert_ne!(a_generator.next(b).dial_id, b_generator.next(a).dial_id);
    }

    #[test]
    fn dial_id_epoch_prevents_restart_reuse() {
        let local = endpoint_id();
        let remote = endpoint_id();

        let before_restart = DialId::from_initiator_sequence(local, remote, [1; 16], 0);
        let after_restart = DialId::from_initiator_sequence(local, remote, [2; 16], 0);

        assert_ne!(before_restart, after_restart);
    }
}
