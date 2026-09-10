// Classic worker: importScripts is required by the non-modularised PDFium glue, and it
// does not exist in a module worker.
//
// Everything heavy happens here so the page thread never blocks -- which is also what
// makes the panic test meaningful: when the wasm module aborts, this worker dies and
// the page has to notice.
importScripts("bridge.js");
importScripts("spike_option1.js");

let ready = false;

async function ensureReady() {
  if (ready) return;
  // `no-modules` target exposes a global `wasm_bindgen` that both loads and inits.
  await wasm_bindgen("spike_option1_bg.wasm");
  await self.initEngines(self.location.href.replace(/\/[^/]*$/, ""));
  ready = true;
}

self.onmessage = async (e) => {
  const { id, cmd, engine, name } = e.data;
  try {
    await ensureReady();

    if (cmd === "ping") {
      self.postMessage({ id, ok: true, version: wasm_bindgen.version() });
      return;
    }

    if (cmd === "panic") {
      // Aborts the whole wasm instance. No reply is possible; the page must time out
      // or see the error event. This is the behaviour apps/web/CLAUDE.md describes.
      wasm_bindgen.deliberate_panic();
      self.postMessage({ id, ok: false, unreachable: true });
      return;
    }

    if (cmd === "open") {
      const res = await fetch(`corpus/${name}`);
      const bytes = new Uint8Array(await res.arrayBuffer());

      const t0 = performance.now();
      const outcome =
        engine === "pdfium"
          ? wasm_bindgen.open_with_pdfium(bytes)
          : wasm_bindgen.open_with_qpdf(bytes);
      const ms = performance.now() - t0;

      self.postMessage({
        id,
        ok: true,
        engine,
        name,
        inputBytes: bytes.length,
        pages: outcome.pages,
        error: outcome.error,
        isOk: outcome.is_ok,
        ms: +ms.toFixed(1),
        engineHeap: self.engineHeapBytes(),
        rustHeap: wasm_bindgen.__wasm?.memory?.buffer?.byteLength ?? 0,
      });
      return;
    }

    self.postMessage({ id, ok: false, error: `unknown cmd ${cmd}` });
  } catch (err) {
    self.postMessage({ id, ok: false, threw: String(err).slice(0, 200) });
  }
};
