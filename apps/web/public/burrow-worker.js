// The engine worker: one instance of each Emscripten module, and one Rust module driving
// both.
//
// WHY A CLASSIC WORKER
//
// The prebuilt `pdfium.js` is not modularised -- its state lives in worker-global `var`s --
// so it can only be loaded with `importScripts`, which exists only in a classic worker. A
// classic worker cannot `import` an ES module, which is why the Rust package is built
// `wasm-pack --target no-modules` and why the bridge imports resolve from global scope.
//
// THE MEMOISED PROMISE (ADR 0006 requirement 1)
//
// `ready` holds the init **promise**, not a boolean and not the result. Spike 0001's HIGH
// finding was a guard flag set *after* an `await`: two concurrent messages each ran init to
// completion, the second `importScripts` rebound every pdfium glue global to a new instance,
// and the bridge kept a reference to the first. Calls then read and wrote the other
// instance's linear memory. The sandbox holds, so nothing escapes -- parses just come back
// confidently wrong, which is worse. Memoising the promise makes the second caller await
// the first init instead of starting another.
//
// WHY THE ENGINES ARE FETCHED HERE AND NOT BY THE GLUE
//
// The CSP names the two engine `.wasm` URLs exactly and forbids everything else, so the
// glue's own `locateFile`/fetch path must never run. We fetch with `integrity`, so the
// bytes are pinned as well as the origin, compile with `WebAssembly.compileStreaming`, and
// hand the compiled module in through `Module.instantiateWasm`. The modules are built
// `-sENVIRONMENT=web,worker`, which removes the Node branches entirely.
//
// NEVER ECHO WHAT THE MODULE THREW
//
// ADR 0009: a trap or an Emscripten `abort()` surfaces as a JS exception whose text is
// panic output and can carry input-derived bytes. The catch below binds nothing and reports
// a fixed, content-free `Internal`.

"use strict";

importScripts("/burrow-bridge.js");

/** @type {Promise<void>|null} The memoised init promise. Never a boolean. */
let ready = null;

/** Fetch one engine artifact under the CSP, with its integrity pinned. */
async function fetchEngine(entry) {
  const response = await fetch(entry.url, { integrity: entry.integrity });
  if (!response.ok) {
    throw new Error("engine fetch failed");
  }
  return response;
}

/**
 * Supply an already-compiled module to Emscripten instead of letting it fetch.
 *
 * Returning `{}` tells Emscripten the instantiation is asynchronous and that `done` will be
 * called; returning the exports synchronously is the other supported shape and is not what
 * a streaming compile can do.
 */
function instantiateWith(compiled) {
  return (imports, done) => {
    WebAssembly.instantiate(compiled, imports).then((instance) => done(instance, compiled));
    return {};
  };
}

/**
 * Silence both modules at the JavaScript layer.
 *
 * ROADMAP item 7's other half. qpdf is also silenced at the C++ layer, by the Rust code, but
 * these two stubs catch anything either module writes on its own account -- including
 * Emscripten's own abort diagnostics, which quote addresses.
 */
const silent = { print: () => {}, printErr: () => {} };

async function init(engines) {
  // qpdf first. It is MODULARIZE'd, so it introduces exactly one global and cannot disturb
  // anything; doing it before pdfium keeps the window in which pdfium's glue globals are
  // live as short as possible.
  const [qpdfResponse, pdfiumResponse, burrowResponse] = await Promise.all([
    fetchEngine(engines.qpdfWasm),
    fetchEngine(engines.pdfiumWasm),
    fetchEngine(engines.burrowWasm),
  ]);
  const [qpdfCompiled, pdfiumCompiled, burrowCompiled] = await Promise.all([
    WebAssembly.compileStreaming(qpdfResponse),
    WebAssembly.compileStreaming(pdfiumResponse),
    WebAssembly.compileStreaming(burrowResponse),
  ]);

  importScripts(engines.qpdfJs.url);
  const qpdfModule = await createQpdfModule({
    ...silent,
    instantiateWasm: instantiateWith(qpdfCompiled),
  });

  // PDFium's glue reads a pre-existing global `Module` for its configuration, and writes
  // its state back into worker globals. This is the load that must happen exactly once.
  self.Module = {
    ...silent,
    instantiateWasm: instantiateWith(pdfiumCompiled),
  };
  const pdfiumReady = new Promise((resolve) => {
    self.Module.onRuntimeInitialized = () => resolve(self.Module);
  });
  importScripts(engines.pdfiumJs.url);
  const pdfiumModule = await pdfiumReady;
  pdfiumModule._FPDF_InitLibrary();

  __burrow_attach(pdfiumModule, qpdfModule);

  // The Rust module last: its imports are the `__burrow_*` globals, which now exist.
  //
  // The already-compiled module is handed in rather than a URL. Passing a URL would make
  // wasm-bindgen's glue fetch it -- without integrity, and as a second request the CSP would
  // have to permit anyway. This way every .wasm the worker loads goes through the same
  // fetch-with-integrity path.
  importScripts(engines.burrowJs.url);
  await wasm_bindgen({ module_or_path: burrowCompiled });
}

function ensureReady(engines) {
  // `??=` assigns the promise, not its result, and only when there is not one already.
  ready ??= init(engines);
  return ready;
}

/** Build the Rust-side limits object from the five numbers the page sent. */
function limitsOf(limits) {
  return new wasm_bindgen.WebLimits(
    BigInt(limits.maxInputBytes),
    BigInt(limits.maxMemoryBytes),
    BigInt(limits.maxDurationMs),
    BigInt(limits.maxPages),
    BigInt(limits.maxPixels),
  );
}

/** Flatten a `Reply` for `postMessage`. Reads fields; decides nothing. */
function replyToMessage(id, reply) {
  return {
    id,
    ok: reply.ok,
    kind: reply.kind,
    // Computed in Rust. The page acts on this; it does not recompute it from `kind`.
    fatal: reply.fatal,
    message: reply.message,
    pages: Number(reply.pages),
    limit: reply.limit,
    requested: Number(reply.requested),
    allowed: Number(reply.allowed),
  };
}

self.onmessage = async (event) => {
  const request = event.data;

  if (request.type === "init") {
    try {
      await ensureReady(request.engines);
      self.postMessage({ id: request.id, ready: true });
    } catch {
      // Not readable, not reported: see the header. A worker that cannot initialise is not
      // usable, so the page discards it.
      self.postMessage({ id: request.id, ready: false, fatal: true, kind: "Internal" });
    }
    return;
  }

  let reply;
  try {
    await ensureReady(request.engines);
    const bytes = new Uint8Array(request.bytes);
    const password = request.password ? new Uint8Array(request.password) : undefined;
    const limits = limitsOf(request.limits);

    reply =
      request.op === "structure_check"
        ? wasm_bindgen.structure_check(bytes, password, Boolean(request.attemptRecovery), limits)
        : wasm_bindgen.page_count(bytes, password, limits);
  } catch {
    // A trap, or an Emscripten abort. Deliberately no binding: the text is never inspected,
    // logged or forwarded. This instance is poisoned -- ADR 0009 -- so the reply is fatal
    // and the page will terminate the worker.
    self.postMessage({
      id: request.id,
      ok: false,
      kind: "Internal",
      fatal: true,
      message: "internal error",
      pages: 0,
      limit: "",
      requested: 0,
      allowed: 0,
    });
    return;
  }

  self.postMessage(replyToMessage(request.id, reply));
};
