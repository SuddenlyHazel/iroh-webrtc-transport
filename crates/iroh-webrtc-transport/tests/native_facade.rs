#![cfg(all(
    feature = "native",
    not(all(target_family = "wasm", target_os = "unknown"))
))]

use std::time::Duration;

use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey, Watcher, endpoint::presets};
use iroh_webrtc_transport::{
    NativeWebRtcIrohNode, WebRtcDialOptions, WebRtcNodeConfig, config::WebRtcSessionConfig,
};
use n0_future::{task, time::timeout as future_timeout};

const ALPN: &[u8] = b"noop/iroh-webrtc-transport/native-facade/4";
const PING: &[u8] = b"ping";
const PONG: &[u8] = b"pong";

#[test]
fn native_facade_type_is_public_importable() {
    fn assert_type_export<T>() -> &'static str {
        std::any::type_name::<T>()
    }

    let _ = assert_type_export::<NativeWebRtcIrohNode>();
}

#[tokio::test]
async fn native_facade_binds_without_ice_servers_and_exposes_endpoint_info() -> anyhow::Result<()> {
    let node = native_node(SecretKey::generate()).await?;

    assert_eq!(node.endpoint_id(), node.endpoint_addr().id);
    assert_eq!(node.endpoint_id(), node.endpoint().id());
    assert_eq!(
        node.webrtc_capability_addr().endpoint_id,
        node.endpoint_id()
    );

    node.close().await;
    Ok(())
}

#[tokio::test]
async fn native_facade_dials_and_accepts_ping_pong_over_webrtc() -> anyhow::Result<()> {
    let client = native_node(SecretKey::generate()).await?;
    let server = native_node(SecretKey::generate()).await?;
    let server_addr = endpoint_addr(&server).await?;

    let accept_server = server.clone();
    let accept = task::spawn(async move {
        let connection = timeout("accept facade connection", accept_server.accept(ALPN)).await?;
        for _ in 0..3 {
            let (mut send, mut recv) =
                timeout("accept bidirectional stream", connection.accept_bi()).await?;
            let ping = timeout("read ping", recv.read_to_end(64)).await?;
            assert_eq!(ping, PING);
            timeout("write pong", send.write_all(PONG)).await?;
            send.finish()?;
        }
        connection.closed().await;
        anyhow::Ok(())
    });

    let connection = timeout(
        "dial facade connection",
        client.dial(server_addr, ALPN, WebRtcDialOptions::webrtc_only()),
    )
    .await?;
    for _ in 0..3 {
        let (mut send, mut recv) =
            timeout("open bidirectional stream", connection.open_bi()).await?;
        timeout("write ping", send.write_all(PING)).await?;
        send.finish()?;
        let pong = timeout("read pong", recv.read_to_end(64)).await?;
        assert_eq!(pong, PONG);
    }

    connection.close(0u32.into(), b"native facade test complete");
    timeout("join accept task", accept).await??;
    client.close().await;
    server.close().await;
    Ok(())
}

#[tokio::test]
async fn native_webrtc_only_node_rejects_plain_iroh_application_dial() -> anyhow::Result<()> {
    let server =
        native_node_with_options(SecretKey::generate(), WebRtcDialOptions::webrtc_only()).await?;
    let client = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let server_addr = endpoint_addr(&server).await?;

    let error = timeout(
        "plain Iroh application dial should fail",
        client.connect(server_addr, ALPN),
    )
    .await
    .expect_err("plain Iroh dial unexpectedly reached WebRTC-only native app ALPN");

    assert!(
        error.to_string().contains("ALPN")
            || error.to_string().contains("closed")
            || error.to_string().contains("application")
            || error.to_string().contains("protocol"),
        "unexpected plain Iroh dial error: {error:#}"
    );

    client.close().await;
    server.close().await;
    Ok(())
}

async fn native_node(secret_key: SecretKey) -> anyhow::Result<NativeWebRtcIrohNode> {
    native_node_with_options(secret_key, WebRtcDialOptions::default()).await
}

async fn native_node_with_options(
    secret_key: SecretKey,
    default_dial_options: WebRtcDialOptions,
) -> anyhow::Result<NativeWebRtcIrohNode> {
    let config = WebRtcNodeConfig {
        session: WebRtcSessionConfig::no_ice_servers(),
        default_dial_options,
        ..WebRtcNodeConfig::default()
    };
    NativeWebRtcIrohNode::builder(config, secret_key.clone())
        .endpoint_builder(Endpoint::builder(presets::N0).relay_mode(RelayMode::Disabled))
        .accept_facade(ALPN)
        .spawn()
        .await
}

async fn endpoint_addr(node: &NativeWebRtcIrohNode) -> anyhow::Result<EndpointAddr> {
    let endpoint = node.endpoint();
    future_timeout(Duration::from_secs(10), async move {
        let mut watch = endpoint.watch_addr();
        loop {
            let addr = watch.get();
            if addr
                .addrs
                .iter()
                .any(|addr| addr.is_ip() || addr.is_relay())
            {
                return anyhow::Ok(addr);
            }
            watch.updated().await?;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for endpoint address"))?
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
