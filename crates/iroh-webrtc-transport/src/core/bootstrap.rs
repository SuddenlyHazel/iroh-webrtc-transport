use std::{future::Future, time::Duration};

use anyhow::{Context, Result, bail};
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::{Connection, RecvStream, SendStream},
};
use n0_future::time::timeout;

use crate::core::signaling::WebRtcSignal;

pub const WEBRTC_BOOTSTRAP_ALPN: &[u8] = b"noop/iroh/webrtc/bootstrap/1";
pub const DEFAULT_MAX_BOOTSTRAP_MESSAGE_LEN: usize = 64 * 1024;
pub const DEFAULT_BOOTSTRAP_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootstrapConfig {
    pub max_message_len: usize,
    pub operation_timeout: Option<Duration>,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            max_message_len: DEFAULT_MAX_BOOTSTRAP_MESSAGE_LEN,
            operation_timeout: Some(DEFAULT_BOOTSTRAP_OPERATION_TIMEOUT),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BootstrapStream {
    connection: Connection,
    config: BootstrapConfig,
}

impl BootstrapStream {
    pub async fn connect(
        endpoint: &Endpoint,
        remote: impl Into<EndpointAddr>,
        config: BootstrapConfig,
    ) -> Result<Self> {
        let connection = with_bootstrap_timeout(
            config.operation_timeout,
            "opening Iroh WebRTC bootstrap connection",
            endpoint.connect(remote, WEBRTC_BOOTSTRAP_ALPN),
        )
        .await?
        .context("failed to open Iroh WebRTC bootstrap connection")?;
        Ok(Self { connection, config })
    }

    pub fn from_connection(connection: Connection, config: BootstrapConfig) -> Self {
        Self { connection, config }
    }

    pub async fn open_channel(&self) -> Result<BootstrapChannel> {
        let (send, recv) = with_bootstrap_timeout(
            self.config.operation_timeout,
            "opening WebRTC bootstrap stream",
            self.connection.open_bi(),
        )
        .await?
        .context("failed to open WebRTC bootstrap stream")?;
        Ok(BootstrapChannel {
            send,
            recv,
            config: self.config,
        })
    }

    pub async fn accept_channel(&self) -> Result<BootstrapChannel> {
        let (send, recv) = with_bootstrap_timeout(
            self.config.operation_timeout,
            "accepting WebRTC bootstrap stream",
            self.connection.accept_bi(),
        )
        .await?
        .context("failed to accept WebRTC bootstrap stream")?;
        Ok(BootstrapChannel {
            send,
            recv,
            config: self.config,
        })
    }
}

#[derive(Debug)]
pub struct BootstrapChannel {
    send: SendStream,
    recv: RecvStream,
    config: BootstrapConfig,
}

impl BootstrapChannel {
    pub fn split(self) -> (BootstrapSignalSender, BootstrapSignalReceiver) {
        (
            BootstrapSignalSender {
                send: self.send,
                config: self.config,
            },
            BootstrapSignalReceiver {
                recv: self.recv,
                config: self.config,
            },
        )
    }
}

#[derive(Debug)]
pub struct BootstrapSignalSender {
    send: SendStream,
    config: BootstrapConfig,
}

impl BootstrapSignalSender {
    pub async fn send_signal(&mut self, signal: &WebRtcSignal) -> Result<()> {
        write_signal_with_timeout(&mut self.send, signal, self.config).await
    }

    pub fn finish(&mut self) -> std::result::Result<(), iroh::endpoint::ClosedStream> {
        self.send.finish()
    }
}

#[derive(Debug)]
pub struct BootstrapSignalReceiver {
    recv: RecvStream,
    config: BootstrapConfig,
}

impl BootstrapSignalReceiver {
    pub async fn recv_signal(&mut self) -> Result<WebRtcSignal> {
        read_signal_with_timeout(&mut self.recv, self.config).await
    }
}

async fn write_signal_with_timeout(
    send: &mut SendStream,
    signal: &WebRtcSignal,
    config: BootstrapConfig,
) -> Result<()> {
    let bytes = encode_signal(signal, config.max_message_len)?;
    with_bootstrap_timeout(
        config.operation_timeout,
        "writing WebRTC bootstrap signal",
        send.write_all(&bytes),
    )
    .await?
    .context("failed to write WebRTC bootstrap signal")?;
    Ok(())
}

async fn read_signal_with_timeout(
    recv: &mut RecvStream,
    config: BootstrapConfig,
) -> Result<WebRtcSignal> {
    let mut len = [0u8; 4];
    with_bootstrap_timeout(
        config.operation_timeout,
        "reading WebRTC bootstrap signal length",
        recv.read_exact(&mut len),
    )
    .await?
    .context("failed to read WebRTC bootstrap signal length")?;
    let len = u32::from_be_bytes(len) as usize;
    if len > config.max_message_len {
        bail!(
            "WebRTC bootstrap signal length {len} exceeds maximum {}",
            config.max_message_len
        );
    }

    let mut bytes = vec![0u8; len];
    with_bootstrap_timeout(
        config.operation_timeout,
        "reading WebRTC bootstrap signal body",
        recv.read_exact(&mut bytes),
    )
    .await?
    .context("failed to read WebRTC bootstrap signal body")?;
    decode_signal_body(&bytes)
}

pub fn encode_signal(signal: &WebRtcSignal, max_message_len: usize) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(signal).context("failed to encode WebRTC bootstrap signal")?;
    if body.len() > max_message_len {
        bail!(
            "WebRTC bootstrap signal length {} exceeds maximum {max_message_len}",
            body.len()
        );
    }
    if body.len() > u32::MAX as usize {
        bail!("WebRTC bootstrap signal is too large");
    }

    let mut bytes = Vec::with_capacity(4 + body.len());
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&body);
    Ok(bytes)
}

#[cfg(test)]
fn decode_signal_frame(bytes: &[u8], max_message_len: usize) -> Result<WebRtcSignal> {
    if bytes.len() < 4 {
        bail!("WebRTC bootstrap signal frame is truncated");
    }
    let len = u32::from_be_bytes(bytes[..4].try_into().expect("slice length checked")) as usize;
    if len > max_message_len {
        bail!("WebRTC bootstrap signal length {len} exceeds maximum {max_message_len}");
    }
    if bytes.len() != 4 + len {
        bail!("WebRTC bootstrap signal frame length mismatch");
    }
    decode_signal_body(&bytes[4..])
}

fn decode_signal_body(bytes: &[u8]) -> Result<WebRtcSignal> {
    serde_json::from_slice(bytes).context("failed to decode WebRTC bootstrap signal")
}

async fn with_bootstrap_timeout<T>(
    duration: Option<Duration>,
    operation: &'static str,
    future: impl Future<Output = T>,
) -> Result<T> {
    match duration {
        Some(duration) => timeout(duration, future).await.with_context(|| {
            format!(
                "timed out after {} while {operation}",
                format_duration(duration)
            )
        }),
        None => Ok(future.await),
    }
}

fn format_duration(duration: Duration) -> String {
    if duration.as_millis() < 1_000 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{:.3}s", duration.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::signaling::{BootstrapTransportIntent, DialId};

    fn endpoint_id() -> iroh::EndpointId {
        iroh::SecretKey::generate().public()
    }

    #[test]
    fn signal_frame_round_trips() {
        let signal = WebRtcSignal::dial_request_with_alpn(
            DialId([7; 16]),
            endpoint_id(),
            endpoint_id(),
            2,
            "",
            BootstrapTransportIntent::WebRtcPreferred,
        );

        let encoded = encode_signal(&signal, DEFAULT_MAX_BOOTSTRAP_MESSAGE_LEN).unwrap();
        let decoded = decode_signal_frame(&encoded, DEFAULT_MAX_BOOTSTRAP_MESSAGE_LEN).unwrap();

        assert_eq!(decoded, signal);
    }

    #[test]
    fn oversized_signal_frame_is_rejected() {
        let signal = WebRtcSignal::Offer {
            dial_id: DialId([1; 16]),
            from: endpoint_id(),
            to: endpoint_id(),
            generation: 1,
            sdp: "x".repeat(256),
        };

        assert!(encode_signal(&signal, 32).is_err());
    }

    #[test]
    fn truncated_signal_frame_is_rejected() {
        assert!(decode_signal_frame(&[0, 0, 0], DEFAULT_MAX_BOOTSTRAP_MESSAGE_LEN).is_err());
        assert!(
            decode_signal_frame(&[0, 0, 0, 10, 1, 2], DEFAULT_MAX_BOOTSTRAP_MESSAGE_LEN).is_err()
        );
    }

    #[test]
    fn default_config_enables_operation_timeout() {
        assert_eq!(
            BootstrapConfig::default().operation_timeout,
            Some(DEFAULT_BOOTSTRAP_OPERATION_TIMEOUT)
        );
    }

    #[tokio::test]
    async fn bootstrap_timeout_reports_operation_context() {
        let err = with_bootstrap_timeout(
            Some(Duration::from_millis(1)),
            "testing bootstrap timeout",
            std::future::pending::<()>(),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("testing bootstrap timeout"));
        assert!(err.to_string().contains("timed out after"));
    }
}
