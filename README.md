# iroh-webrtc-transport

Iroh + WebRTC = Even More Weird Web Stuff

> Whoa, hold on there partner!

1. This is still very much experimental
2. The API surface may still change as the transport model settles
3. This isn't "generic." I've laid a lot of my own opinions in here.

## Ahh, opinions?

The browser runtime is direct main-thread Wasm now. There is no browser worker
runtime, worker command bridge, `MessagePort` control channel, or DataChannel
transfer between threads.

Secret material, Iroh endpoint/session state, WebRTC negotiation, application
protocol handlers, streams, benchmark helpers, and DataChannel packet I/O all
live behind the browser facade in one runtime. That is the architecture this
crate is carrying forward.

## So what do you get for free?

The goal is that application code should not need to know much about WebRTC ceremony. You ask for an Iroh-style connection, and the crate handles the browser mess behind it.

- Browser Wasm facade that exposes Iroh-style endpoint IDs, dial, accept, and stream APIs
- WebRTC channel bootstrapping with signaling using Iroh's relays
- Native WebRTC implementation so browser clients can connect directly to native clients
- Endpoints, ALPNs, connections, and streams; it's still "just Iroh."

I tried my hardest to have the crate bury all the WebRTC setup and Iroh custom transport glue.
