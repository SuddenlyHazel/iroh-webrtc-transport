# iroh-webrtc-transport

Iroh + WebRTC = Even More Weird Web Stuff

> Whoa, hold on there partner!

1. This is still very much experimental
2. This is still in the process of being cleaned up
3. This isn't "generic." I've laid a lot of my own opinions in here.

## Ahh, opinions?

The current experimental branch intentionally drops the browser worker split.
Secret material, Iroh endpoint/session state, WebRTC negotiation, and
DataChannel packet I/O all live in the browser main-thread Wasm runtime so the
implementation can test whether removing the RPC boundary is simpler overall.

## So what do you get for free?

The goal is that application code should not need to know much about WebRTC ceremony. You ask for an Iroh-style connection, and the crate handles the browser mess behind it.

- WebRTC channel bootstrapping with signaling using Iroh's relays
- Native WebRTC implementation so browser clients can connect directly to native clients
- Endpoints, ALPNs, connections, and streams; it's still "just Iroh."

I tried my hardest to have the crate bury all the WebRTC setup and Iroh custom transport glue.
