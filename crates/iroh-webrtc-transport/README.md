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
`browser` feature on `wasm32-unknown-unknown`. The browser facade starts the
worker runtime, exposes the local endpoint ID, and provides dial, accept,
bidirectional stream, and benchmark helpers.

The browser architecture is split by browser API availability:

- Main-thread Wasm owns `RTCPeerConnection`, SDP offer/answer handling, ICE
  candidate creation, `RTCDataChannel` creation, and browser WebRTC callbacks.
- Worker Wasm owns Iroh endpoint/session state, application protocol state,
  packet queues, and the transferred `RTCDataChannel` after attachment.

The `browser_app!` macro can export a main-thread app entry point and a worker
entry point from one Wasm module. The example at
`examples/rust-browser-ping-pong` uses this path.

## Module Map

- `browser`: browser Wasm facade, main-thread RTC setup, worker startup, and
  worker command bridge.
- `config`: public configuration knobs, including STUN URLs, queue sizing,
  frame sizing, and DataChannel backpressure thresholds.
- `facade`: shared facade configuration and dial options.
- `native`: native/server WebRTC session backend, enabled by the `native`
  feature on non-wasm targets.
- `transport`: Iroh custom transport integration.

## Feature Flags

- `native`: enables the native WebRTC backend on non-wasm targets.
- `browser-main-thread`: enables browser `RTCPeerConnection` bindings and the
  Rust browser facade on `wasm32-unknown-unknown`.
- `browser-worker`: enables the browser worker entry point on
  `wasm32-unknown-unknown`.
- `browser`: enables both browser feature sets.

The default feature set is currently `browser-main-thread` plus `native`.

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

## Browser Worker Model

For browser applications that run Iroh/Wasm in a worker, keep
`RTCPeerConnection` orchestration on the browser main thread. The WebRTC spec
exposes [`RTCPeerConnection`][webrtc-peerconnection] and
[`RTCIceCandidate`][webrtc-ice-candidate] to `Window`, while
[`RTCDataChannel`][webrtc-datachannel] is exposed to both `Window` and
`DedicatedWorker` and is transferable.

This distinction matters because WebRTC setup is not just the DataChannel. The
DataChannel only opens after peer connection creation, SDP negotiation, ICE
candidate exchange, candidate-pair selection, and DTLS/SCTP setup. Those setup
steps depend on APIs that are not consistently exposed to workers across the
browsers this crate targets.

The crate's browser path therefore uses a main-thread RTC control bridge plus a
worker-owned Iroh runtime. Transferred `RTCDataChannel` support is required for
the browser product path.

[webrtc-peerconnection]: https://w3c.github.io/webrtc-pc/#rtcpeerconnection-interface
[webrtc-ice-candidate]: https://w3c.github.io/webrtc-pc/#rtcicecandidate-interface
[webrtc-datachannel]: https://w3c.github.io/webrtc-pc/#rtcdatachannel
