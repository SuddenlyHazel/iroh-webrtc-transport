use iroh::EndpointId;
use iroh_base::CustomAddr;

use crate::error::{Error, Result};

/// Iroh custom transport identifier used by this WebRTC transport.
///
/// Iroh routes packets for custom transports through [`CustomAddr`] values.
/// This identifier lets the Iroh endpoint distinguish WebRTC custom addresses
/// from custom addresses owned by other transports.
///
/// The value is a stable mnemonic tag rather than a generated number:
/// `0x69_77_72_74_63` is ASCII for `iwrtc` ("Iroh WebRTC"), followed by a
/// small suffix for this first address format. It is not a security boundary or
/// registry-assigned value; it only needs to remain stable and avoid collisions
/// with other custom transports registered on the same endpoint.
pub const WEBRTC_TRANSPORT_ID: u64 = 0x6977_7274_6300_0001;

const VERSION: u8 = 1;
const KIND_CAPABILITY: u8 = 1;
const KIND_SESSION: u8 = 2;
const ENDPOINT_ID_LEN: usize = 32;
const SESSION_ID_LEN: usize = 16;

/// The role a [`WebRtcAddr`] plays in the transport.
///
/// These are not IP socket addresses. WebRTC does not expose a stable address
/// that Iroh can route packets to: the real path is owned by ICE, DTLS/SCTP,
/// and an `RTCDataChannel`. Instead, the custom address is a compact routing
/// key used by the Iroh custom transport adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcAddrKind {
    /// Advertises that an endpoint can participate in WebRTC bootstrap.
    ///
    /// A capability address is useful for discovery and local address
    /// publication. It should not be used as a packet send destination because
    /// it does not identify an active WebRTC data-channel session.
    Capability,

    /// Routes packets into one active WebRTC session.
    ///
    /// The session id normally comes from the WebRTC bootstrap `dial_id`. It
    /// disambiguates multiple simultaneous WebRTC peer connections for the same
    /// Iroh endpoint.
    Session { session_id: [u8; SESSION_ID_LEN] },
}

/// Virtual WebRTC address encoded into Iroh's [`CustomAddr`] format.
///
/// `WebRtcAddr` exists because Iroh's custom transport API is packet oriented:
/// when `noq` wants to transmit a packet it gives the custom sender a
/// destination [`CustomAddr`]. For UDP-like transports that address can map to a
/// socket address. For WebRTC, there is no durable socket address to encode.
///
/// This type therefore encodes:
///
/// - the remote Iroh [`EndpointId`], preserving the Iroh identity being routed
///   to; and
/// - either a capability marker or a session id that identifies the active
///   `RTCDataChannel`/native data-channel attachment.
///
/// The normal application-level API should hide this type. Most consumers
/// should dial by endpoint id and let the worker runtime create session
/// addresses internally. Consumers only need to construct `WebRtcAddr` directly
/// when manually wiring [`WebRtcTransport`](crate::WebRtcTransport),
/// [`SessionHub`](crate::SessionHub), and Iroh `EndpointAddr` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebRtcAddr {
    /// Iroh identity this virtual WebRTC address belongs to.
    pub endpoint_id: EndpointId,

    /// Whether this address advertises WebRTC support or targets a concrete
    /// session.
    pub kind: WebRtcAddrKind,
}

impl WebRtcAddr {
    /// Creates an address that advertises WebRTC capability for an endpoint.
    ///
    /// This is appropriate for `watch_local_addrs()` style publication. It is
    /// not a valid destination for actual packet sends.
    pub fn capability(endpoint_id: EndpointId) -> Self {
        Self {
            endpoint_id,
            kind: WebRtcAddrKind::Capability,
        }
    }

    /// Creates an address for routing packets to one active WebRTC session.
    ///
    /// Custom senders accept this form as a destination. The data-channel
    /// attachment path uses the same session id in packet frames to reject
    /// traffic for the wrong session.
    pub fn session(endpoint_id: EndpointId, session_id: [u8; SESSION_ID_LEN]) -> Self {
        Self {
            endpoint_id,
            kind: WebRtcAddrKind::Session { session_id },
        }
    }

    /// Encodes this virtual address into Iroh's custom-address wire type.
    pub fn to_custom_addr(self) -> CustomAddr {
        let mut data = Vec::with_capacity(2 + ENDPOINT_ID_LEN + SESSION_ID_LEN);
        data.push(VERSION);
        match self.kind {
            WebRtcAddrKind::Capability => {
                data.push(KIND_CAPABILITY);
                data.extend_from_slice(self.endpoint_id.as_bytes());
            }
            WebRtcAddrKind::Session { session_id } => {
                data.push(KIND_SESSION);
                data.extend_from_slice(self.endpoint_id.as_bytes());
                data.extend_from_slice(&session_id);
            }
        }
        CustomAddr::from_parts(WEBRTC_TRANSPORT_ID, &data)
    }

    /// Decodes a WebRTC virtual address from an Iroh custom address.
    ///
    /// Returns an error if the custom transport id, version, kind, or encoded
    /// lengths do not match this transport's address format.
    pub fn from_custom_addr(addr: &CustomAddr) -> Result<Self> {
        if addr.id() != WEBRTC_TRANSPORT_ID {
            return Err(Error::InvalidAddr("unexpected custom transport id"));
        }
        let data = addr.data();
        if data.len() < 2 + ENDPOINT_ID_LEN {
            return Err(Error::InvalidAddr("address data too short"));
        }
        if data[0] != VERSION {
            return Err(Error::InvalidAddr("unsupported address version"));
        }

        let endpoint_bytes: &[u8; ENDPOINT_ID_LEN] = data[2..2 + ENDPOINT_ID_LEN]
            .try_into()
            .map_err(|_| Error::InvalidAddr("invalid endpoint id length"))?;
        let endpoint_id = EndpointId::from_bytes(endpoint_bytes)
            .map_err(|_| Error::InvalidAddr("invalid endpoint id bytes"))?;

        let kind = match data[1] {
            KIND_CAPABILITY => {
                if data.len() != 2 + ENDPOINT_ID_LEN {
                    return Err(Error::InvalidAddr("capability address has trailing data"));
                }
                WebRtcAddrKind::Capability
            }
            KIND_SESSION => {
                if data.len() != 2 + ENDPOINT_ID_LEN + SESSION_ID_LEN {
                    return Err(Error::InvalidAddr("session address has invalid length"));
                }
                let session_id = data[2 + ENDPOINT_ID_LEN..]
                    .try_into()
                    .map_err(|_| Error::InvalidAddr("invalid session id length"))?;
                WebRtcAddrKind::Session { session_id }
            }
            _ => return Err(Error::InvalidAddr("unknown address kind")),
        };

        Ok(Self { endpoint_id, kind })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint_id() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    #[test]
    fn capability_addr_round_trips() {
        let addr = WebRtcAddr::capability(endpoint_id());
        let custom = addr.to_custom_addr();

        assert_eq!(WebRtcAddr::from_custom_addr(&custom).unwrap(), addr);
    }

    #[test]
    fn session_addr_round_trips() {
        let session_id = [9; SESSION_ID_LEN];
        let addr = WebRtcAddr::session(endpoint_id(), session_id);
        let custom = addr.to_custom_addr();

        assert_eq!(WebRtcAddr::from_custom_addr(&custom).unwrap(), addr);
    }

    #[test]
    fn rejects_wrong_transport_id() {
        let custom = CustomAddr::from_parts(42, &[VERSION, KIND_CAPABILITY]);

        assert!(matches!(
            WebRtcAddr::from_custom_addr(&custom),
            Err(Error::InvalidAddr("unexpected custom transport id"))
        ));
    }

    #[test]
    fn rejects_capability_with_trailing_data() {
        let mut data = Vec::new();
        data.push(VERSION);
        data.push(KIND_CAPABILITY);
        data.extend_from_slice(endpoint_id().as_bytes());
        data.push(0);
        let custom = CustomAddr::from_parts(WEBRTC_TRANSPORT_ID, &data);

        assert!(matches!(
            WebRtcAddr::from_custom_addr(&custom),
            Err(Error::InvalidAddr("capability address has trailing data"))
        ));
    }
}
