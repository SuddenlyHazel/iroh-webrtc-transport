# iroh-webrtc-transport

WebRTC-backed custom transport primitives and facades for Iroh.

This crate is experimental and currently `publish = false`. It provides the
Rust pieces used to bootstrap a WebRTC `RTCDataChannel`, attach that channel to
Iroh's custom transport interface, and expose normal Iroh-style connections and
streams to application code.

## Which API Should I Use?

For application code, start with the facades:

| Goal | Start here |
| --- | --- |
| Browser Wasm app | `iroh_webrtc_transport::browser::BrowserWebRtcNode` |
| Native/server app | `iroh_webrtc_transport::NativeWebRtcIrohNode` |
| Shared dial options | `iroh_webrtc_transport::WebRtcDialOptions` |
| Session, STUN, frame, queue, and DataChannel tuning | `iroh_webrtc_transport::config` |

For lower-level integration, `transport::{WebRtcTransport, configure_endpoint}`
and `native::NativeWebRtcSession` remain available. Those APIs are for callers
that want to wire their own bootstrap/signaling flow.

## Basic Native Shape

With the `native` feature enabled, the facade owns an Iroh bootstrap endpoint,
a WebRTC custom-transport application endpoint, and the native WebRTC session
lifecycle:

```rust
use iroh::SecretKey;
use iroh_webrtc_transport::{
    NativeWebRtcIrohNode, WebRtcDialOptions, WebRtcNodeConfig,
};

# async fn example(remote: iroh::EndpointAddr) -> anyhow::Result<()> {
let node = NativeWebRtcIrohNode::spawn(
    WebRtcNodeConfig::default(),
    SecretKey::generate(),
)
.await?;

let connection = node
    .dial(remote, b"example/alpn/1", WebRtcDialOptions::webrtc_only())
    .await?;
# let _ = connection;
# Ok(())
# }
```

`NativeWebRtcIrohNode::accept(alpn)` returns ordinary Iroh `Connection` values
for inbound application protocols.

## Basic Browser Shape

Browser applications should use `browser::BrowserWebRtcNode` with the
`browser` feature on `wasm32-unknown-unknown`. The browser facade owns the
main-thread runtime, exposes the local endpoint ID, and provides dial, accept,
bidirectional stream, typed protocol, and benchmark helpers.

The browser architecture is direct and single-runtime:

- Main-thread Wasm owns Iroh endpoint/session state, application protocol
  state, packet queues, `RTCPeerConnection`, SDP/ICE, and `RTCDataChannel`
  packet I/O.
- There is no browser worker runtime, `MessagePort` RTC control channel, worker
  command bridge, worker compatibility feature, or DataChannel transfer between
  threads.

The `browser_app!` macro exports the main-thread app entry point.

## Module Map

- `browser`: public browser Wasm facade, typed protocol registry, browser
  streams, benchmark helpers, and `browser_app!` entry-point macro.
- `browser_runtime`: internal direct browser runtime for endpoint/session state,
  bootstrap signaling, RTC lifecycle, packet queues, and DataChannel pumping.
- `config`: public configuration knobs, including STUN URLs, queue sizing,
  frame sizing, and DataChannel backpressure thresholds.
- `facade`: shared facade configuration and dial options.
- `native`: native/server WebRTC session backend, enabled by the `native`
  feature on non-wasm targets.
- `transport`: Iroh custom transport integration.

## Feature Flags

- `native`: enables the native WebRTC backend on non-wasm targets.
- `browser`: enables the browser main-thread runtime on
  `wasm32-unknown-unknown`.
- `browser-main-thread`: implementation feature pulled in by `browser`.
  Application code should enable `browser`, not this feature directly.

The default feature set is currently `browser` plus `native`.

## Transport Intent

WebRTC is attempted through an explicit dial intent:

- `WebRtcDialOptions::iroh_relay()`: skip WebRTC and use Iroh's normal
  relay-capable application dial path.
- `WebRtcDialOptions::webrtc_preferred()`: attempt WebRTC first, then fall back
  to Iroh relay if WebRTC cannot be promoted.
- `WebRtcDialOptions::webrtc_only()`: require the WebRTC custom path. If the
  connection selects a relay/IP path or the wrong WebRTC session path, the dial
  fails.

`webrtc_preferred()` is the default.

## Bootstrap And Hole Punching

The WebRTC path uses Iroh to establish an authenticated bootstrap stream between
endpoint IDs. A small bootstrap protocol sends the WebRTC offer, answer, ICE
candidates, and selected transport intent over that stream.

WebRTC ICE does the candidate gathering and connectivity checks:

```text
Iroh bootstrap stream
  -> exchange offer / answer / ICE candidates
  -> gather host and STUN server-reflexive candidates
  -> ICE checks candidate pairs
  -> selected candidate pair opens the DataChannel path
```

If a direct WebRTC path opens, Iroh/noq packets are sent through the
`RTCDataChannel` using this crate's custom transport. If WebRTC cannot open, the
dial intent decides whether to fall back to Iroh relay or surface an error.

TURN is intentionally not modeled by this crate today. `WebRtcIceConfig` accepts
only `stun:` and `stuns:` URLs. Relay fallback, when allowed, is Iroh relay
fallback rather than TURN.

## Encryption And Overhead

The WebRTC path carries framed Iroh/noq packets over an `RTCDataChannel`.
DataChannels run over SCTP over DTLS. Iroh/noq still owns endpoint identity,
connection authentication, and encrypted connection semantics above the custom
transport.

That means the WebRTC path has layered encryption and framing:

```text
Iroh/noq encrypted packets
  -> iroh-webrtc-transport frame
  -> RTCDataChannel / SCTP
  -> DTLS
  -> ICE-selected path
```

This preserves the Iroh protocol model, but it is not a zero-cost transport.

## Tuning Knobs

- `WebRtcIceConfig`: configure direct-only STUN URLs or disable configured ICE
  servers for localhost/same-LAN tests.
- `WebRtcFrameConfig`: configure the maximum packet payload accepted by the
  DataChannel frame encoder/decoder. The default is 1200 bytes.
- `WebRtcDataChannelConfig`: configure DataChannel buffered amount thresholds.
- `WebRtcQueueConfig`: configure inbound and outbound custom transport queue
  capacity.
- `WebRtcSessionConfig`: bundle ICE, frame, DataChannel, and local ICE queue
  settings for browser/native WebRTC sessions.

## Browser Main-Thread Model

The browser runtime keeps browser state in main-thread Wasm by design. That
includes endpoint keys, session state, application protocol handlers, WebRTC
negotiation, DataChannel handlers, and packet queues.

This crate no longer includes the older worker-shaped architecture. Browser
applications enter through `BrowserWebRtcNode` and the `browser_app!` macro; the
runtime calls browser WebRTC APIs directly and pumps Iroh/noq packets through
the selected `RTCDataChannel`.

[webrtc-peerconnection]: https://w3c.github.io/webrtc-pc/#rtcpeerconnection-interface
[webrtc-ice-candidate]: https://w3c.github.io/webrtc-pc/#rtcicecandidate-interface
[webrtc-datachannel]: https://w3c.github.io/webrtc-pc/#rtcdatachannel
