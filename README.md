# iroh-webrtc-transport

Iroh + WebRTC = Even More Weird Web Stuff

> Whoa, hold on there partner!

1. This is still very much experimental
2. This is still in the process of being cleaned up
3. This isn't "generic." I've laid a lot of my own opinions in here.

## Ahh, opinions?

Just one really: Secret material must live in a web worker and not remain inside the main browser thread.

What this means in practice is that there is a pretty large RPC protocol to facilitate communication between the main application code and the worker thread.

Ideally, everything could just live inside the worker thread. Unfortunately, this is not possible today because the browser does not expose a bunch of APIs inside the worker context.

## So what do you get for free?

The goal is that application code should not need to know much about WebRTC ceremony. You ask for an Iroh-style connection, and the crate handles the browser mess behind it.

- WebRTC channel bootstrapping with signaling using Iroh's relays
- Native WebRTC implementation so browser clients can connect directly to native clients
- Endpoints, ALPNs, connections, and streams; it's still "just Iroh."

I tried my hardest to have the crate bury all the WebRTC setup and Iroh custom transport glue.

