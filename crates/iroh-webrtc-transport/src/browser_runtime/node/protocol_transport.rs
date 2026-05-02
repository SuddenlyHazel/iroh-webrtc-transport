use super::*;
use iroh::endpoint::{AfterHandshakeOutcome, BeforeConnectOutcome, EndpointHooks};
use std::sync::{Mutex, Weak};

#[derive(Debug, Clone)]
pub(super) struct BrowserProtocolTransportHook {
    inner: Weak<Mutex<BrowserRuntimeNodeInner>>,
}

impl EndpointHooks for BrowserProtocolTransportHook {
    async fn before_connect<'a>(
        &'a self,
        remote_addr: &'a EndpointAddr,
        alpn: &'a [u8],
    ) -> BeforeConnectOutcome {
        let alpn = String::from_utf8_lossy(alpn).into_owned();
        let Some(inner) = self.inner.upgrade() else {
            return BeforeConnectOutcome::Reject;
        };
        let (registered, transport_intent, prepare_tx) = {
            let inner = inner.lock().expect("browser runtime node mutex poisoned");
            (
                inner.browser_protocols.contains_alpn(&alpn),
                inner.protocol_transport_intent,
                inner.protocol_transport_prepare_tx.clone(),
            )
        };
        if !registered || !transport_intent.uses_webrtc() {
            return BeforeConnectOutcome::Accept;
        }
        if endpoint_addr_has_webrtc_session(remote_addr) {
            return BeforeConnectOutcome::Accept;
        }

        let Some(prepare_tx) = prepare_tx else {
            return if transport_intent.allows_iroh_relay_fallback() {
                BeforeConnectOutcome::Accept
            } else {
                BeforeConnectOutcome::Reject
            };
        };
        let (response, result) = oneshot::channel();
        let request = ProtocolTransportPrepareRequest {
            remote: remote_addr.id,
            alpn,
            transport_intent,
            response,
        };
        if prepare_tx.send(request).await.is_err() {
            return if transport_intent.allows_iroh_relay_fallback() {
                BeforeConnectOutcome::Accept
            } else {
                BeforeConnectOutcome::Reject
            };
        }
        match result.await {
            Ok(Ok(())) => BeforeConnectOutcome::Accept,
            Ok(Err(err)) => {
                tracing::debug!(
                    target: "iroh_webrtc_transport::browser_runtime::protocol_transport",
                    %err,
                    "failed to prepare browser protocol WebRTC transport"
                );
                if transport_intent.allows_iroh_relay_fallback() {
                    BeforeConnectOutcome::Accept
                } else {
                    BeforeConnectOutcome::Reject
                }
            }
            Err(_) => {
                if transport_intent.allows_iroh_relay_fallback() {
                    BeforeConnectOutcome::Accept
                } else {
                    BeforeConnectOutcome::Reject
                }
            }
        }
    }

    async fn after_handshake<'a>(
        &'a self,
        conn: &'a iroh::endpoint::ConnectionInfo,
    ) -> AfterHandshakeOutcome {
        let alpn = String::from_utf8_lossy(conn.alpn()).into_owned();
        let Some(inner) = self.inner.upgrade() else {
            return reject_non_webrtc_protocol_connection();
        };
        let (registered, transport_intent) = {
            let inner = inner.lock().expect("browser runtime node mutex poisoned");
            (
                inner.browser_protocols.contains_alpn(&alpn),
                inner.protocol_transport_intent,
            )
        };
        if !registered || transport_intent != BootstrapTransportIntent::WebRtcOnly {
            return AfterHandshakeOutcome::Accept;
        }
        if selected_path_is_webrtc_session(conn) {
            AfterHandshakeOutcome::Accept
        } else {
            reject_non_webrtc_protocol_connection()
        }
    }
}

impl BrowserRuntimeNode {
    pub(super) fn protocol_transport_hook(&self) -> BrowserProtocolTransportHook {
        BrowserProtocolTransportHook {
            inner: Arc::downgrade(&self.inner),
        }
    }

    pub(in crate::browser_runtime) fn add_protocol_transport_session_addr(
        &self,
        remote: EndpointId,
        dial_id: DialId,
    ) -> BrowserRuntimeResult<()> {
        let inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        if inner.closed {
            return Err(BrowserRuntimeError::closed());
        }
        let custom_addr = WebRtcAddr::session(remote, dial_id.0).to_custom_addr();
        inner
            .protocol_transport_lookup
            .add_endpoint_info(EndpointAddr::from_parts(
                remote,
                [TransportAddr::Custom(custom_addr)],
            ));
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::browser_runtime) fn protocol_transport_endpoint_addr(
        &self,
        remote: EndpointId,
    ) -> Option<EndpointAddr> {
        let inner = self
            .inner
            .lock()
            .expect("browser runtime node mutex poisoned");
        inner
            .protocol_transport_lookup
            .get_endpoint_info(remote)
            .map(|info| info.into_endpoint_addr())
    }
}

fn endpoint_addr_has_webrtc_session(remote_addr: &EndpointAddr) -> bool {
    remote_addr.addrs.iter().any(|addr| {
        let TransportAddr::Custom(custom_addr) = addr else {
            return false;
        };
        matches!(
            WebRtcAddr::from_custom_addr(custom_addr),
            Ok(WebRtcAddr {
                kind: crate::core::addr::WebRtcAddrKind::Session { .. },
                ..
            })
        )
    })
}

fn selected_path_is_webrtc_session(conn: &iroh::endpoint::ConnectionInfo) -> bool {
    let Some(path) = conn.selected_path() else {
        return false;
    };
    let TransportAddr::Custom(custom_addr) = path.remote_addr() else {
        return false;
    };
    matches!(
        WebRtcAddr::from_custom_addr(custom_addr),
        Ok(WebRtcAddr {
            kind: crate::core::addr::WebRtcAddrKind::Session { .. },
            ..
        })
    )
}

fn reject_non_webrtc_protocol_connection() -> AfterHandshakeOutcome {
    AfterHandshakeOutcome::Reject {
        error_code: 0u32.into(),
        reason: b"browser protocol requires WebRTC custom transport".to_vec(),
    }
}
