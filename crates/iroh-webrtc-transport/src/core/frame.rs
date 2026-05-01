use crate::error::{Error, Result};

/// Four-byte frame marker for Iroh-over-WebRTC datagram messages.
///
/// `IWRT` is scoped to this crate's private DataChannel format.
pub const FRAME_MAGIC: [u8; 4] = *b"IWRT";

/// Binary frame format version.
pub const FRAME_VERSION: u8 = 4;

/// Maximum payload bytes accepted for one decoded NOQ transmit.
pub const DEFAULT_MAX_PAYLOAD_LEN: usize = u16::MAX as usize;

// Header layout:
//   magic[4] + version[1] + flags[1] + session_id[16]
//   + segment_size[4] + payload_len[4]
const HEADER_LEN: usize = 4 + 1 + 1 + 16 + 4 + 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcPacketFrame {
    pub flags: u8,
    pub session_id: [u8; 16],
    pub segment_size: Option<usize>,
    pub payload: Vec<u8>,
}

impl WebRtcPacketFrame {
    pub fn encode(&self, max_payload_len: usize) -> Result<Vec<u8>> {
        if self.payload.is_empty() {
            return Err(Error::InvalidFrame("payload is empty"));
        }
        if self.payload.len() > max_payload_len {
            return Err(Error::PayloadTooLarge {
                actual: self.payload.len(),
                max: max_payload_len,
            });
        }
        let payload_len: u32 = self
            .payload
            .len()
            .try_into()
            .map_err(|_| Error::InvalidFrame("payload length exceeds u32"))?;
        let segment_size = normalized_segment_size(self.segment_size, self.payload.len())?;

        let mut out = Vec::with_capacity(HEADER_LEN + self.payload.len());
        out.extend_from_slice(&FRAME_MAGIC);
        out.push(FRAME_VERSION);
        out.push(self.flags);
        out.extend_from_slice(&self.session_id);
        out.extend_from_slice(&segment_size.to_be_bytes());
        out.extend_from_slice(&payload_len.to_be_bytes());
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    pub fn decode(bytes: &[u8], max_payload_len: usize) -> Result<Self> {
        let header = decode_header(bytes)?;
        if bytes.len() != HEADER_LEN + header.payload_len {
            return Err(Error::InvalidFrame("payload length mismatch"));
        }
        if header.payload_len == 0 {
            return Err(Error::InvalidFrame("payload is empty"));
        }
        if header.payload_len > max_payload_len {
            return Err(Error::PayloadTooLarge {
                actual: header.payload_len,
                max: max_payload_len,
            });
        }

        Ok(Self {
            flags: header.flags,
            session_id: header.session_id,
            segment_size: header.segment_size,
            payload: bytes[HEADER_LEN..].to_vec(),
        })
    }

    pub fn session_id(bytes: &[u8]) -> Result<[u8; 16]> {
        Ok(decode_header(bytes)?.session_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameHeader {
    flags: u8,
    session_id: [u8; 16],
    segment_size: Option<usize>,
    payload_len: usize,
}

fn decode_header(bytes: &[u8]) -> Result<FrameHeader> {
    if bytes.len() < HEADER_LEN {
        return Err(Error::InvalidFrame("frame too short"));
    }
    if bytes[..4] != FRAME_MAGIC {
        return Err(Error::InvalidFrame("bad magic"));
    }
    if bytes[4] != FRAME_VERSION {
        return Err(Error::InvalidFrame("unsupported version"));
    }

    let flags = bytes[5];
    let session_id = bytes[6..22]
        .try_into()
        .map_err(|_| Error::InvalidFrame("invalid session id length"))?;
    let segment_size = u32::from_be_bytes(
        bytes[22..26]
            .try_into()
            .map_err(|_| Error::InvalidFrame("invalid segment size field"))?,
    );
    let payload_len = u32::from_be_bytes(
        bytes[26..30]
            .try_into()
            .map_err(|_| Error::InvalidFrame("invalid payload length field"))?,
    ) as usize;
    let segment_size = match segment_size {
        0 => None,
        size => {
            let size = size as usize;
            if size >= payload_len {
                return Err(Error::InvalidFrame(
                    "segment size is not smaller than payload",
                ));
            }
            Some(size)
        }
    };

    Ok(FrameHeader {
        flags,
        session_id,
        segment_size,
        payload_len,
    })
}

fn normalized_segment_size(segment_size: Option<usize>, payload_len: usize) -> Result<u32> {
    let Some(segment_size) = segment_size else {
        return Ok(0);
    };
    if segment_size == 0 {
        return Err(Error::InvalidFrame("segment size is zero"));
    }
    if segment_size >= payload_len {
        return Ok(0);
    }
    segment_size
        .try_into()
        .map_err(|_| Error::InvalidFrame("segment size exceeds u32"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_datagram_round_trips() {
        let frame = WebRtcPacketFrame {
            flags: 3,
            session_id: [1; 16],
            segment_size: None,
            payload: b"hello".to_vec(),
        };

        let encoded = frame.encode(DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        assert_eq!(
            WebRtcPacketFrame::decode(&encoded, DEFAULT_MAX_PAYLOAD_LEN).unwrap(),
            frame
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [9; 16],
            segment_size: None,
            payload: b"hello".to_vec(),
        };
        let mut encoded = frame.encode(DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        encoded[0] = b'X';

        assert!(matches!(
            WebRtcPacketFrame::decode(&encoded, DEFAULT_MAX_PAYLOAD_LEN),
            Err(Error::InvalidFrame("bad magic"))
        ));
    }

    #[test]
    fn rejects_truncated_payload() {
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [7; 16],
            segment_size: None,
            payload: b"hello".to_vec(),
        };
        let mut encoded = frame.encode(DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        encoded.pop();

        assert!(matches!(
            WebRtcPacketFrame::decode(&encoded, DEFAULT_MAX_PAYLOAD_LEN),
            Err(Error::InvalidFrame("payload length mismatch"))
        ));
    }

    #[test]
    fn rejects_oversize_payload() {
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [2; 16],
            segment_size: None,
            payload: vec![0; DEFAULT_MAX_PAYLOAD_LEN + 1],
        };

        assert!(matches!(
            frame.encode(DEFAULT_MAX_PAYLOAD_LEN),
            Err(Error::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn parses_session_id_without_payload_validation() {
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [4; 16],
            segment_size: None,
            payload: b"hello".to_vec(),
        };
        let encoded = frame.encode(DEFAULT_MAX_PAYLOAD_LEN).unwrap();

        assert_eq!(WebRtcPacketFrame::session_id(&encoded).unwrap(), [4; 16]);
    }

    #[test]
    fn segmented_transmit_round_trips() {
        let frame = WebRtcPacketFrame {
            flags: 0,
            session_id: [5; 16],
            segment_size: Some(4),
            payload: b"aaaabbbbcc".to_vec(),
        };

        let encoded = frame.encode(DEFAULT_MAX_PAYLOAD_LEN).unwrap();
        assert_eq!(
            WebRtcPacketFrame::decode(&encoded, DEFAULT_MAX_PAYLOAD_LEN).unwrap(),
            frame
        );
    }
}
