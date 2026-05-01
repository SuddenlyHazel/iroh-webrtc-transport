# iroh-webrtc-transport

Reusable WebRTC-backed custom transport primitives for Iroh.

## Which API Should I Use?

Use the grouped modules as your public entry points:

| Goal | Start here |
| --- | --- |
| Add the transport to an Iroh endpoint | `iroh_webrtc_transport::transport` |
| Tune STUN, queues, frames, or backpressure | `iroh_webrtc_transport::config` |
| Build native/server WebRTC support | `iroh_webrtc_transport::native` |
| Build browser wasm WebRTC support | `iroh_webrtc_transport::browser` |

For native/server code, the basic shape is:

```rust
use iroh_webrtc_transport::{
    config::WebRtcTransportConfig,
    transport::{WebRtcTransport, configure_endpoint},
};

let transport = WebRtcTransport::new(WebRtcTransportConfig::default());
let hub = transport.session_hub();
let builder = configure_endpoint(builder, transport);
```

With the `native` feature enabled, use `native::NativeWebRtcSession` to create
or answer the native WebRTC PeerConnection, exchange offer/answer/ICE over a
bootstrap stream, and attach the opened DataChannel to the same `SessionHub`.

For browser code, use `browser::BrowserWebRtcNode`. The facade starts the
worker, exposes the local endpoint ID, and routes dial/accept/stream/benchmark
commands. The crate keeps `RTCPeerConnection`, SDP/ICE, and
`RTCDataChannel` creation on the browser main thread, while worker Wasm owns
Iroh endpoint/session state and attaches transferred data channels before
packet I/O starts.

## Module Map

- `config`: public configuration knobs, including direct-only STUN
  configuration, queue sizing, frame sizing, and DataChannel backpressure.
- `transport`: Iroh custom transport integration.
- `browser`: browser Wasm node facade, main-thread RTC setup, worker startup,
  and command bridge.
- `native`: native/server WebRTC session backend, enabled by the `native`
  feature on non-wasm targets.
- `facade`: target-neutral facade configuration and dial options.

## Encryption And Overhead

The WebRTC path carries framed Iroh/noq packets over an `RTCDataChannel`.
WebRTC DataChannels run over SCTP over DTLS, so the WebRTC layer is forcing
transport security underneath the Iroh/noq encrypted connection.

Iroh/noq still owns the endpoint identity, connection authentication, and encrypted connection
semantics above the custom transport.

But, this means we're forced to have layered encryption and framing..

```text
Iroh/noq encrypted packets
  -> iroh-webrtc-transport frame
  -> RTCDataChannel / SCTP
  -> DTLS
  -> ICE-selected path
```

This preserves the security and protocol model which let's us speak "Iroh". But, yes, it is not a zero-cost
transport.

## Hole Punching

We use Iroh relay dial path to create an authenticated bootstrap stream between
endpoint IDs. A thin WebRTC bootstrap protocol then sends offer/answer and ICE
candidate messages over that stream.

**WebRTC ICE does the actual hole punching:**

```text
Iroh relay/bootstrap stream
  -> exchange offer / answer / ICE candidates
  -> each peer gathers host and STUN server-reflexive candidates
  -> ICE connectivity checks race candidate pairs
  -> selected candidate pair opens the DataChannel path
```

In the direct path, STUN tells each peer what public address/port its NAT has
mapped for the WebRTC socket. ICE then tests candidate pairs from both sides.
If a pair succeeds, the DataChannel uses that path directly. If no direct pair
succeeds, the dialer's transport intent decides whether the application may use
Iroh relay or must fail.

References:

- [ICE, RFC 8445](https://datatracker.ietf.org/doc/html/rfc8445): candidate
  gathering, pairing, and connectivity checks.
- [STUN, RFC 8489](https://datatracker.ietf.org/doc/html/rfc8489): discovers
  server-reflexive addresses through NATs.
- [TURN, RFC 8656](https://datatracker.ietf.org/doc/html/rfc8656): relays
  traffic when a direct candidate pair cannot be established.

## Transport Intent

True peer-to-peer is the preferred path, but it is not guaranteed. ICE can fail
to find a direct candidate pair on restrictive NATs, enterprise networks,
carrier networks, or UDP-blocked paths. Developers should choose what happens
when the direct WebRTC path cannot be promoted into an open DataChannel.

The browser worker protocol carries an explicit `transportIntent` on every
bootstrap session:

- `irohRelay`: skip WebRTC and use Iroh's relay-capable application dial path.
- `webrtcPreferred`: attempt WebRTC first, then close the failed RTC attempt and
  use Iroh relay if WebRTC cannot be promoted.
- `webrtcOnly`: require WebRTC. If WebRTC cannot open a transferred
  DataChannel, surface the error to the application.

The JavaScript dial option `transport` maps directly to this intent. Bootstrap
signals must include `transportIntent`; missing intent is rejected.

> I created this so I wouldn't feel icky sending things like browser first internet radio traffic entirely though Iroh's relays. Be a respectful consumer: host your own relays, or [work with them](https://www.iroh.computer/pricing)!

TURN is intentionally not included. It's WebRTC P2P or fallback on Iroh. `WebRtcIceConfig` only accepts `stun:` and `stuns:` URLs.

## Tuning Knobs

- `WebRtcIceConfig`: configure direct-only STUN URLs. TURN URLs (see above).
- `WebRtcFrameConfig`: configure the maximum packet payload accepted by the
  DataChannel frame encoder/decoder. Default is 1200 bytes.
- `WebRtcBackpressureConfig`: configure DataChannel buffered amount thresholds
  and native polling interval.
- `WebRtcQueueConfig`: configure inbound and outbound custom transport queue
  capacity.
- `WebRtcSessionConfig`: bundle ICE, frame, backpressure, and local ICE queue
  sizing for browser/native WebRTC sessions.

## Feature Flags

- `native`: enables the native WebRTC backend on non-wasm targets.
- `browser-main-thread`: enables browser `RTCPeerConnection` bindings and the
  Rust browser facade on `wasm32-unknown-unknown`.
- `browser-worker`: enables the browser worker entry point on
  `wasm32-unknown-unknown`.

## Browser Worker Model

For browser applications that run Iroh/Wasm in a worker, keep
`RTCPeerConnection` orchestration on the browser main thread. The WebRTC spec
exposes [`RTCPeerConnection`][webrtc-peerconnection] and
[`RTCIceCandidate`][webrtc-ice-candidate] to `Window`, while
[`RTCDataChannel`][webrtc-datachannel] is exposed to both `Window` and
`DedicatedWorker` and is transferable.

Practically, this splits browser applications into two roles:

- Main-thread Wasm owns `RTCPeerConnection`, SDP offer/answer handling, ICE
  candidate creation, `RTCDataChannel` creation, and the browser event
  callbacks for connection state.
- Worker Wasm owns the Iroh runtime, application protocol state, session state,
  packet queues, and the transferred `RTCDataChannel` after attachment.

This distinction matters because WebRTC setup is not just the DataChannel. The
DataChannel only opens after `RTCPeerConnection` has created an offer or answer,
applied the remote session description, gathered ICE candidates, exchanged
remote ICE candidates, selected a candidate pair, and completed DTLS/SCTP setup.
Those negotiation steps depend on **APIs that are not exposed to workers** in the
spec, and browser support is especially strict on Safari and iOS. 

The two Wasm entry points communicate over a dedicated `MessagePort`.
Transferred `RTCDataChannel` support is mandatory for the product browser path.
The crate no longer includes a browser packet bridge or browser-owned session
harness.

[webrtc-peerconnection]: https://w3c.github.io/webrtc-pc/#rtcpeerconnection-interface
[webrtc-ice-candidate]: https://w3c.github.io/webrtc-pc/#rtcicecandidate-interface
[webrtc-datachannel]: https://w3c.github.io/webrtc-pc/#rtcdatachannel
