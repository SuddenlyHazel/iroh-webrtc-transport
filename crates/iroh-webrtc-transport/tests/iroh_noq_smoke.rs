#![cfg(all(
    feature = "native",
    not(all(target_family = "wasm", target_os = "unknown"))
))]

use std::time::Duration;

use iroh::{
    Endpoint, EndpointAddr, RelayMode, SecretKey, TransportAddr,
    endpoint::presets,
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_webrtc_transport::{
    config::WebRtcTransportConfig,
    native::{
        LocalIceEvent, NativeWebRtcSession, NativeWebRtcSessionConfig, WEBRTC_DATA_CHANNEL_LABEL,
        WebRtcAddr,
    },
    transport::{WebRtcTransport, configure_endpoint},
};
use n0_future::{
    task::{self, JoinHandle},
    time::timeout as future_timeout,
};

const ALPN: &[u8] = b"noop/iroh-webrtc-transport/smoke/6";
const PING: &[u8] = b"ping";
const PONG: &[u8] = b"pong";

#[derive(Debug, Clone)]
struct PingPong;

impl ProtocolHandler for PingPong {
    async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let ping = recv.read_to_end(64).await.map_err(AcceptError::from_err)?;
        assert_eq!(ping, PING);
        send.write_all(PONG).await.map_err(AcceptError::from_err)?;
        send.finish()?;
        connection.closed().await;
        Ok(())
    }
}

#[tokio::test]
async fn iroh_endpoints_complete_alpn_ping_pong_over_native_webrtc() -> anyhow::Result<()> {
    let client_key = SecretKey::generate();
    let server_key = SecretKey::generate();
    let session_id = *b"phase-6-smoke-ok";

    let client_addr = WebRtcAddr::session(client_key.public(), session_id).to_custom_addr();
    let server_addr = WebRtcAddr::session(server_key.public(), session_id).to_custom_addr();

    let client_transport = transport(client_addr.clone());
    let server_transport = transport(server_addr.clone());
    let client_hub = client_transport.session_hub();
    let server_hub = server_transport.session_hub();
    let config = NativeWebRtcSessionConfig::no_ice_servers();
    let offerer = NativeWebRtcSession::new_offerer_with_config(
        client_hub,
        server_addr.clone(),
        session_id,
        WEBRTC_DATA_CHANNEL_LABEL,
        config.clone(),
    )
    .await?;
    let answerer = NativeWebRtcSession::new_answerer_with_config(
        server_hub,
        client_addr.clone(),
        session_id,
        config,
    )
    .await?;

    let offer = timeout("create native WebRTC offer", offerer.create_offer()).await?;
    timeout("apply native WebRTC offer", answerer.apply_offer(offer)).await?;
    let offerer_ice = task::spawn(relay_local_ice(offerer.clone(), answerer.clone()));

    let answer = timeout("create native WebRTC answer", answerer.create_answer()).await?;
    timeout("apply native WebRTC answer", offerer.apply_answer(answer)).await?;
    let answerer_ice = task::spawn(relay_local_ice(answerer.clone(), offerer.clone()));

    timeout("relay offerer ICE", join_ice_relay(offerer_ice)).await?;
    timeout("relay answerer ICE", join_ice_relay(answerer_ice)).await?;

    let client = endpoint(client_key, client_transport).await?;
    let server = endpoint(server_key.clone(), server_transport).await?;
    let router = Router::builder(server).accept(ALPN, PingPong).spawn();

    let conn = future_timeout(
        Duration::from_secs(10),
        client.connect(endpoint_addr(server_key.public(), server_addr), ALPN),
    )
    .await??;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(PING).await?;
    send.finish()?;

    let pong = future_timeout(Duration::from_secs(10), recv.read_to_end(64)).await??;
    assert_eq!(pong, PONG);

    conn.close(0u32.into(), b"smoke complete");
    future_timeout(Duration::from_secs(10), router.shutdown()).await??;
    offerer.close().await;
    answerer.close().await;
    Ok(())
}

#[tokio::test]
async fn iroh_endpoints_ping_pong_with_dynamic_webrtc_session_addrs() -> anyhow::Result<()> {
    let client_key = SecretKey::generate();
    let server_key = SecretKey::generate();
    let session_id = *b"dynamic-smoke-ok";

    let client_capability_addr = WebRtcAddr::capability(client_key.public()).to_custom_addr();
    let server_capability_addr = WebRtcAddr::capability(server_key.public()).to_custom_addr();
    let client_addr = WebRtcAddr::session(client_key.public(), session_id).to_custom_addr();
    let server_addr = WebRtcAddr::session(server_key.public(), session_id).to_custom_addr();

    let client_transport = transport(client_capability_addr);
    let server_transport = transport(server_capability_addr);
    let client_hub = client_transport.session_hub();
    let server_hub = server_transport.session_hub();
    let config = NativeWebRtcSessionConfig::no_ice_servers();
    let offerer = NativeWebRtcSession::new_offerer_with_config(
        client_hub,
        server_addr.clone(),
        session_id,
        WEBRTC_DATA_CHANNEL_LABEL,
        config.clone(),
    )
    .await?;
    let answerer = NativeWebRtcSession::new_answerer_with_config(
        server_hub,
        client_addr.clone(),
        session_id,
        config,
    )
    .await?;

    let offer = timeout("create native WebRTC offer", offerer.create_offer()).await?;
    timeout("apply native WebRTC offer", answerer.apply_offer(offer)).await?;
    let offerer_ice = task::spawn(relay_local_ice(offerer.clone(), answerer.clone()));

    let answer = timeout("create native WebRTC answer", answerer.create_answer()).await?;
    timeout("apply native WebRTC answer", offerer.apply_answer(answer)).await?;
    let answerer_ice = task::spawn(relay_local_ice(answerer.clone(), offerer.clone()));

    timeout("relay offerer ICE", join_ice_relay(offerer_ice)).await?;
    timeout("relay answerer ICE", join_ice_relay(answerer_ice)).await?;

    let client = endpoint(client_key, client_transport.clone()).await?;
    let server = endpoint(server_key.clone(), server_transport.clone()).await?;
    client_transport.advertise_local_addr(client_addr);
    server_transport.advertise_local_addr(server_addr.clone());
    let router = Router::builder(server).accept(ALPN, PingPong).spawn();

    let conn = future_timeout(
        Duration::from_secs(10),
        client.connect(endpoint_addr(server_key.public(), server_addr), ALPN),
    )
    .await??;
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(PING).await?;
    send.finish()?;

    let pong = future_timeout(Duration::from_secs(10), recv.read_to_end(64)).await??;
    assert_eq!(pong, PONG);

    conn.close(0u32.into(), b"dynamic smoke complete");
    future_timeout(Duration::from_secs(10), router.shutdown()).await??;
    offerer.close().await;
    answerer.close().await;
    Ok(())
}

fn transport(local_addr: iroh_base::CustomAddr) -> WebRtcTransport {
    WebRtcTransport::new(WebRtcTransportConfig {
        local_addr: Some(local_addr),
        ..WebRtcTransportConfig::default()
    })
}

async fn endpoint(
    secret_key: SecretKey,
    transport: WebRtcTransport,
) -> anyhow::Result<iroh::Endpoint> {
    let builder = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports();
    Ok(configure_endpoint(builder, transport).bind().await?)
}

fn endpoint_addr(endpoint_id: iroh::EndpointId, addr: iroh_base::CustomAddr) -> EndpointAddr {
    EndpointAddr::from_parts(endpoint_id, [TransportAddr::Custom(addr)])
}

async fn relay_local_ice(from: NativeWebRtcSession, to: NativeWebRtcSession) -> anyhow::Result<()> {
    loop {
        match from.next_local_ice().await? {
            LocalIceEvent::Candidate(candidate) => to.add_ice_candidate(candidate).await?,
            LocalIceEvent::EndOfCandidates => {
                to.add_end_of_candidates().await?;
                return Ok(());
            }
        }
    }
}

async fn join_ice_relay(handle: JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
    handle.await?
}

async fn timeout<T, E>(
    operation: &'static str,
    future: impl std::future::Future<Output = Result<T, E>>,
) -> anyhow::Result<T>
where
    anyhow::Error: From<E>,
{
    future_timeout(Duration::from_secs(10), future)
        .await
        .map_err(|_| anyhow::anyhow!("{operation} timed out"))?
        .map_err(Into::into)
}
