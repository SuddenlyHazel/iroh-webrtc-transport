use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
export function __iroh_webrtc_create_single_wasm_worker_factory(moduleUrl, wasmModule, workerEntry) {
  if (typeof moduleUrl !== "string" || moduleUrl.length === 0) {
    throw new Error("Iroh WebRTC worker moduleUrl must be a non-empty string");
  }
  if (!(wasmModule instanceof WebAssembly.Module)) {
    throw new Error("Iroh WebRTC worker wasmModule must be a WebAssembly.Module");
  }
  if (typeof workerEntry !== "string" || workerEntry.length === 0) {
    throw new Error("Iroh WebRTC worker entry export must be a non-empty string");
  }

  const workerSource = `
    let started = false;
    const queuedMessages = [];

    const wireError = (message, detail) => ({
      code: "spawnFailed",
      message,
      detail,
    });

    const postBootstrapFailure = (error) => {
      for (const event of queuedMessages) {
        const id = typeof event.data?.id === "number" ? event.data.id : undefined;
        if (id === undefined) {
          continue;
        }
        globalThis.postMessage({
          kind: "response",
          id,
          ok: false,
          error: wireError("failed to bootstrap Iroh WebRTC worker", {
            reason: error instanceof Error ? error.message : String(error),
          }),
        });
      }
      queuedMessages.length = 0;
    };

    const bootstrap = async (event) => {
      if (!started && event.data?.kind !== "iroh-webrtc-worker-bootstrap") {
        queuedMessages.push(event);
        return;
      }
      if (started) {
        return;
      }

      try {
        const { moduleUrl, wasmModule, workerEntry } = event.data ?? {};
        if (typeof moduleUrl !== "string" || moduleUrl.length === 0) {
          throw new Error("missing worker moduleUrl");
        }
        if (!(wasmModule instanceof WebAssembly.Module)) {
          throw new Error("missing worker WebAssembly.Module");
        }
        if (typeof workerEntry !== "string" || workerEntry.length === 0) {
          throw new Error("missing worker entry export");
        }

        const bindings = await import(moduleUrl);
        const init = bindings.default;
        const startWorker = bindings[workerEntry];
        if (typeof init !== "function") {
          throw new Error("wasm-bindgen module default export is not a function");
        }
        if (typeof startWorker !== "function") {
          throw new Error(\`wasm-bindgen module missing worker entry export: \${workerEntry}\`);
        }

        await init({ module_or_path: wasmModule });
        startWorker();
        started = true;
        globalThis.removeEventListener("message", bootstrap);
        for (const queuedEvent of queuedMessages) {
          globalThis.dispatchEvent(new MessageEvent("message", {
            data: queuedEvent.data,
            ports: queuedEvent.ports,
          }));
        }
        queuedMessages.length = 0;
      } catch (error) {
        globalThis.removeEventListener("message", bootstrap);
        postBootstrapFailure(error);
        throw error;
      }
    };

    globalThis.addEventListener("message", bootstrap);
  `;

  const workerUrl = URL.createObjectURL(
    new Blob([workerSource], { type: "text/javascript" }),
  );

  return () => {
    const worker = new Worker(workerUrl, { type: "module" });
    worker.postMessage({
      kind: "iroh-webrtc-worker-bootstrap",
      moduleUrl,
      wasmModule,
      workerEntry,
    });
    return worker;
  };
}
"#)]
extern "C" {
    #[wasm_bindgen(
        js_name = __iroh_webrtc_create_single_wasm_worker_factory,
        catch
    )]
    fn js_create_single_wasm_worker_factory(
        module_url: &str,
        wasm_module: JsValue,
        worker_entry: &str,
    ) -> Result<js_sys::Function, JsValue>;
}

/// Create a worker factory that starts the Iroh WebRTC worker from this Wasm module.
#[wasm_bindgen(js_name = createIrohWebRtcWorkerFactory)]
pub fn create_current_wasm_worker_factory(
    module_url: &str,
    worker_entry: &str,
) -> Result<js_sys::Function, JsValue> {
    create_single_wasm_worker_factory(module_url, wasm_bindgen::module(), worker_entry)
}

/// Create a worker factory that starts the Iroh WebRTC worker from a provided Wasm module.
#[wasm_bindgen(js_name = createIrohWebRtcWorkerFactoryForModule)]
pub fn create_single_wasm_worker_factory(
    module_url: &str,
    wasm_module: JsValue,
    worker_entry: &str,
) -> Result<js_sys::Function, JsValue> {
    js_create_single_wasm_worker_factory(module_url, wasm_module, worker_entry)
}
