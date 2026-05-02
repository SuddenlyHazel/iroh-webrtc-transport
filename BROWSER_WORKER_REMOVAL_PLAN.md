# Browser Worker Removal Execution Plan

## Goal

Finish the direct main-thread browser runtime architecture. Remove remaining worker-era artifacts, command/result-shaped internal APIs, JSON/value adapter leftovers, stale reachability clutter, and misleading module boundaries.

This pass does not preserve the removed browser-worker architecture. There are no compatibility shims, no backwards-compatibility adapters for browser-worker, and no browser automation test runs as part of this execution.

## Target Architecture

The supported architecture is:

```text
BrowserWebRtcNode public facade
  -> direct browser runtime owner
      -> browser node/session state
      -> typed browser RTC registry
      -> typed signaling over WebRtcSignal
      -> typed connection and stream handles
      -> protocol transport preparation where still required
```

There should be no internal worker command bus, no internal JSON result bus, no command/result envelopes, no generic effect dispatcher, no worker vocabulary in domain names, and no "wire" or "payload" layer unless it describes an actual public wire format.

## Exit Criteria

1. `rg "Worker|worker|protocol value|command|payload|wire" crates/iroh-webrtc-transport/src/browser*` has only accepted, documented hits.
2. Browser wasm check, browser wasm tests check, native check, examples check, and native tests pass, or any remaining warning/test gap is explicitly explained.
3. Native test-harness warnings from browser runtime internals are removed or properly cfg'd away.
4. The pass removes real obsolete code, not only renames it; the final report includes net LOC change.
5. The final browser runtime module map is explainable as direct browser node architecture: facade to browser node/session/connection/stream/signaling, with no worker command dispatch layer.

## Milestone 1: Inventory And Remove Proven-Dead Code

Search for remaining worker/wire/payload/command/protocol-value artifacts and unused browser runtime items.

Remove unused constants, fields, helpers, enum variants, tests, and result plumbing proven unreachable.

Commit as a cleanup milestone.

## Milestone 2: Replace Command/Result-Shaped Internals

Remove or replace these browser runtime internals:

- `WorkerSpawnResult`
- `WorkerAcceptOpenResult`
- `WorkerAcceptNextResult`
- `WorkerStreamOpenResult`
- `WorkerStreamAcceptResult`
- `WorkerStreamSendResult`
- `WorkerStreamReceiveResult`
- `WorkerCloseResult`

Replace them with browser-domain objects such as:

- browser node info
- accept registration
- accepted connection
- browser stream handle
- send and receive outcomes
- close outcomes

Only keep a type if it expresses a real domain boundary rather than a command response.

Commit as a cleanup milestone.

## Milestone 3: Collapse Redundant Runtime/Core/Node Ownership Layers

Re-evaluate this chain:

```text
BrowserMainThreadRuntime -> BrowserRuntimeCore -> BrowserRuntimeNode
```

Remove pass-through ownership layers if they are no longer pulling weight. Keep only the smallest architecture that expresses actual direct main-thread runtime ownership.

Commit as a cleanup milestone.

## Milestone 4: Rename Remaining Worker-Named Domain Types After Deletion

Rename remaining real domain types only after obsolete code is deleted:

- `WorkerSessionKey`
- `WorkerSessionLifecycle`
- `WorkerResolvedTransport`
- `WorkerDataChannelAttachmentState`
- `WorkerProtocolTransportPrepareRequest`

Avoid cosmetic churn before deletion is complete.

Commit as a cleanup milestone.

## Milestone 5: Clarify Browser Runtime Module Boundaries

Reshape modules around responsibilities:

- runtime/node ownership
- sessions
- signaling
- connections
- streams
- protocol transport
- JavaScript error conversion

Remove misleading `main_thread` and `wire` naming if it no longer represents the architecture.

Commit as a cleanup milestone.

## Milestone 6: Final Debt Gate

Run artifact scans.

Run formatting and Rust checks/tests that do not require browser automation.

Produce LOC and remaining artifact report.

Commit the final cleanup if needed.
