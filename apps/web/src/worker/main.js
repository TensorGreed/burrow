// The worker's message protocol. Last file in the bundle: everything it uses exists by now.
//
// NEVER ECHO WHAT THE MODULE THREW
//
// ADR 0009: a trap or an Emscripten `abort()` surfaces as a JS exception whose text is panic
// output and can carry input-derived bytes. Every `catch` below binds nothing and reports a
// fixed, content-free `Internal`.
//
// THE MEMOISED PROMISE (ADR 0006 requirement 1)
//
// `ready` holds the init **promise**, not a boolean and not the result. Spike 0001's HIGH
// finding was a guard flag set *after* an `await`: two concurrent messages each ran init to
// completion, and the second load rebound every pdfium glue global while the bridge still
// held the first instance. Calls then read and wrote the other instance's linear memory —
// the sandbox holds, so parses simply come back confidently wrong, which is worse.
//
// Bundling makes that much harder to reintroduce (there is no second `importScripts` to
// make), but the promise is still what serialises two messages arriving before init has
// finished.

// NO "use strict" HERE, deliberately.
//
// It was here and it was INERT: the bundle emits the generated `BURROW_ENGINES` const before
// this file, so the directive is no longer in a directive prologue and has no effect on any
// of the bundle's ~1,000 lines. Leaving it in would be a comment that claims a guarantee the
// code does not have.
//
// Making it real would mean emitting it as the bundle's genuine first statement, which would
// also flip 160 KB of third-party Emscripten glue to strict mode -- a much larger and
// entirely untested change for no benefit we need. What strict mode would buy here is a
// `ReferenceError` on an undeclared assignment, and `tsc -p src/worker` already reports that
// as "Cannot find name" (verified). `src/production-build.test.ts` asserts the bundle's mode
// so this cannot drift back silently.

/** @type {Promise<void>|null} The memoised init promise. Never a boolean. */
let ready = null;

async function init() {
  // qpdf is MODULARIZE'd, so it is a function call rather than a load-time side effect.
  const qpdfModule = await createQpdfModule({
    ...silent,
    instantiateWasm: instantiateFrom("qpdfWasm"),
  });

  // PDFium's glue began instantiating when the bundle was parsed; `Module` is the object the
  // prelude installed, and `onRuntimeInitialized` is how it reports readiness.
  await new Promise((resolve) => {
    if (Module.calledRun) {
      resolve(undefined);
      return;
    }
    const previous = Module.onRuntimeInitialized;
    Module.onRuntimeInitialized = () => {
      if (previous) previous();
      resolve(undefined);
    };
  });

  // THE ONE NARROWING, and the one place the guarantee holds. `Module` is declared as a
  // config because that is what it is when the prelude assigns it; the glue populates the
  // same object with the exports, and `onRuntimeInitialized` (awaited above) is exactly the
  // signal that it has finished. See `EmscriptenConfig` in globals.d.ts.
  const pdfiumModule = /** @type {EmscriptenModule} */ (/** @type {unknown} */ (Module));
  pdfiumModule._FPDF_InitLibrary();

  __burrow_attach(pdfiumModule, qpdfModule);

  // The Rust module last: its imports are the `__burrow_*` globals, which now exist. The
  // compiled module is handed in rather than a URL, so every `.wasm` the worker loads goes
  // through the same integrity-checked fetch.
  const burrowModule = await compiled["burrowWasm"];
  await wasm_bindgen({ module_or_path: burrowModule });
}

function ensureReady() {
  // `??=` assigns the promise, not its result, and only when there is not one already.
  ready ??= init();
  return ready;
}

/**
 * A content-free failure. Used wherever an explanation would be a leak or a guess.
 *
 * @param {number} id
 * @param {string} message
 */
function internalFailure(id, message) {
  return {
    id,
    ok: false,
    kind: "Internal",
    fatal: true,
    message,
    pages: 0,
    limit: "",
    requested: 0,
    allowed: 0,
  };
}

/**
 * Flatten a `Reply` for `postMessage`, then release it. Reads fields; decides nothing.
 *
 * @param {number} id
 * @param {Reply} reply
 */
function drainReply(id, reply) {
  try {
    return {
      id,
      ok: reply.ok,
      kind: reply.kind,
      // Computed in Rust. The page acts on this; it does not recompute it from `kind`.
      fatal: reply.fatal,
      message: reply.message,
      pages: Number(reply.pages),
      limit: reply.limit,
      // Strings, not Numbers: these are `u64` and `estimated_open_bytes` saturates, so a
      // value above 2^53 is reachable and `Number()` would round it. The page is showing a
      // user which ceiling they hit; a wrong number there is a small lie with no upside.
      requested: reply.requested.toString(),
      allowed: reply.allowed.toString(),
    };
  } finally {
    // A wasm-bindgen object is a boxed Rust value in the burrow module's heap, and wasm
    // memory only grows. One leaked reply per operation is a worker that grows forever.
    reply.free();
  }
}

self.onmessage = async (event) => {
  const request = event.data;

  // THE FAIL-CLOSED GUARD.
  //
  // A worker constructed from a plain URL does not inherit the page's CSP and runs with no
  // policy at all -- measured, and the reason this bundle is loaded from a Blob. Refusing to
  // touch a file in that case means a regression is loud, instead of silently removing the
  // browser-enforced half of the guarantee while every test still passes.
  //
  // `POLICED` measures the property directly (a request the policy must refuse) rather than
  // inferring it from the URL scheme, because a Blob worker inherits whatever policy the
  // creating document had -- including none.
  if (!(await POLICED)) {
    self.postMessage(
      internalFailure(
        request.id,
        CREATED_FROM_BLOB
          ? "no content security policy is in force in this worker"
          : "worker was not created from a blob: URL, so it inherits no policy",
      ),
    );
    return;
  }

  if (request.type === "init") {
    try {
      await ensureReady();
      self.postMessage({ id: request.id, ready: true });
    } catch {
      // Not readable, not reported. A worker that cannot initialise is not usable, so the
      // page discards it.
      self.postMessage({ id: request.id, ready: false, fatal: true, kind: "Internal" });
    }
    return;
  }

  let reply = null;
  try {
    await ensureReady();
    const bytes = new Uint8Array(request.bytes);
    const password = request.password ? new Uint8Array(request.password) : undefined;
    // NOT freed here, and that is not an oversight. wasm-bindgen passes a struct argument
    // BY VALUE: the generated glue calls `limits.__destroy_into_raw()` and hands the raw
    // pointer to Rust, which then owns it. Calling `.free()` afterwards is a double free
    // and throws "null pointer passed to rust" -- which manifests as the worker never
    // replying at all, because the throw escapes the message handler.
    //
    // `Reply` is the other way round: it comes back owned, so `drainReply` frees it.
    const limits = new wasm_bindgen.WebLimits(
      BigInt(request.limits.maxInputBytes),
      BigInt(request.limits.maxMemoryBytes),
      BigInt(request.limits.maxDurationMs),
      BigInt(request.limits.maxPages),
      BigInt(request.limits.maxPixels),
    );

    if (request.op === "page_count") {
      reply = wasm_bindgen.page_count(bytes, password, limits);
    } else if (request.op === "structure_check") {
      reply = wasm_bindgen.structure_check(
        bytes,
        password,
        Boolean(request.attemptRecovery),
        limits,
      );
    } else {
      // An unknown op is a bug in the page, not a poisoned engine -- so it is reported
      // without costing a worker. Falling through to `page_count` would have been the
      // one-line version and would silently do the wrong operation.
      self.postMessage({
        id: request.id,
        ok: false,
        kind: "InvalidArgument",
        fatal: false,
        message: "unknown operation",
        pages: 0,
        limit: "",
        requested: "0",
        allowed: "0",
      });
      return;
    }
  } catch {
    // A trap, or an Emscripten abort. Deliberately no binding: the text is never inspected,
    // logged or forwarded. This instance is poisoned (ADR 0009), so the reply is fatal and
    // the page terminates the worker.
    self.postMessage(internalFailure(request.id, "internal error"));
    return;
  }

  self.postMessage(drainReply(request.id, reply));
};
