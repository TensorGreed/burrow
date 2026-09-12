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
    stage: "",
    requested: 0,
    allowed: 0,
    // A worker that failed this way is being discarded anyway, so there is nothing to
    // recycle and no heap reading worth trusting.
    recycle: false,
    pdfiumHeapBytes: "0",
    qpdfHeapBytes: "0",
  };
}

/**
 * Flatten a `WebLimits` for `postMessage`, then release it.
 *
 * Owned when it comes back from `default_limits()`, unlike the one built for an operation --
 * that one is consumed by the call it is passed to. See the note in the message handler.
 *
 * @param {ReturnType<typeof wasm_bindgen.default_limits>} limits
 */
function drainLimits(limits) {
  try {
    return {
      maxInputBytes: Number(limits.max_input_bytes),
      maxMemoryBytes: Number(limits.max_memory_bytes),
      maxDurationMs: Number(limits.max_duration_ms),
      maxPages: Number(limits.max_pages),
      maxPixels: Number(limits.max_pixels),
    };
  } finally {
    limits.free();
  }
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
      // WHICH CHECK FIRED, not just which ceiling. Three different mechanisms enforce
      // `max_memory_bytes`, and ROADMAP item 12's harness compares this across the native and
      // web paths -- the same error kind reached by a different route is a divergence.
      stage: reply.stage,
      // Strings, not Numbers: these are `u64` and `estimated_open_bytes` saturates, so a
      // value above 2^53 is reachable and `Number()` would round it. The page is showing a
      // user which ceiling they hit; a wrong number there is a small lie with no upside.
      requested: reply.requested.toString(),
      allowed: reply.allowed.toString(),
      // THE LIFECYCLE VERDICT, computed in Rust like `fatal` and for the same reason: the
      // threshold is derived from `max_memory_bytes`, and ADR 0009 forbids a binding
      // enforcing any part of `Limits`. Read and forwarded; nothing here compares it
      // against anything.
      recycle: reply.recycle,
      // Strings for the same reason `requested` is: these are `u64`.
      pdfiumHeapBytes: reply.pdfium_heap_bytes.toString(),
      qpdfHeapBytes: reply.qpdf_heap_bytes.toString(),
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
  const policy = await POLICED;
  if (!policy.policed) {
    // The reason travels to the page BY MESSAGE, not to the console. It is a fixed
    // identifier from a closed set -- never engine output, never anything input-derived --
    // and the console is both unread in production and the one place file bytes must never
    // reach.
    self.postMessage(internalFailure(request.id, `no policy in force: ${policy.reason}`));
    return;
  }

  if (request.type === "init") {
    try {
      await ensureReady();
      self.postMessage({
        id: request.id,
        ready: true,
        // Reported here rather than exported as a JS constant: it is a Rust number derived
        // from what the engine modules declare, and a copy on this side would be free to
        // drift from the one that actually decides whether a worker is recycled.
        minConvergingMemoryBytes: wasm_bindgen.min_converging_memory_bytes().toString(),
        // The core's own ceilings. The conformance harness merges a case's overrides onto
        // THESE, because `expectations.json` says an omitted `limits` block means
        // `Limits::DEFAULT` and the native side honours that literally -- a differential
        // harness whose two sides run under different defaults is not comparing what it says.
        defaultLimits: drainLimits(wasm_bindgen.default_limits()),
      });
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

    if (request.op !== "page_count" && request.op !== "structure_check") {
      // BEFORE `limits` is constructed, deliberately. An unknown op is a bug in the page, not
      // a poisoned engine, so it is reported without costing a worker — but returning after
      // building a `WebLimits` would leak it: nothing consumes it on this path, and a
      // wasm-bindgen struct is a boxed Rust value in a heap that never shrinks. Falling
      // through to `page_count` would have been the one-line version and would silently do
      // the wrong operation.
      self.postMessage({
        id: request.id,
        ok: false,
        kind: "InvalidArgument",
        fatal: false,
        message: "unknown operation",
        pages: 0,
        limit: "",
        stage: "",
        requested: "0",
        allowed: "0",
        recycle: false,
        pdfiumHeapBytes: "0",
        qpdfHeapBytes: "0",
      });
      return;
    }

    // THE ACK, AND WHY IT IS HERE RATHER THAN AT THE TOP OF THIS HANDLER.
    //
    // The page's watchdog starts its clock on this message, not on its own `postMessage`.
    // Everything above this line -- the policy guard, and a cold 6.5 MB engine compile that
    // `ensureReady()` may be awaiting -- is start-up, which the page bounds separately. A
    // file must never be blamed for time spent before the worker could look at it.
    //
    // That is the web form of the bug PR 2 fixed natively: a caller delayed behind someone
    // else's work was told its own deadline had expired.
    //
    // It goes out BEFORE `blob.arrayBuffer()` because reading the file IS the operation --
    // a 100 MB blob takes real time and that time is attributable to the file, unlike the
    // engine compile.
    self.postMessage({ id: request.id, ack: true });

    // A Blob, not a transferred ArrayBuffer. Structured clone passes a Blob BY REFERENCE,
    // so the page never materialises the bytes in its own heap and -- the part that
    // matters for recovery -- the caller still holds a usable handle to the same file after
    // a worker is killed. A transferred ArrayBuffer is detached on the page side and gone,
    // which would make "retry on a fresh worker" impossible for exactly the files that
    // needed it.
    //
    // Reading a Blob is a memory read. It issues no request, so no CSP directive is
    // consulted -- asserted rather than assumed by `e2e/zero-requests.spec.ts`, which
    // delivers the whole corpus this way and watches the server's own log.
    const bytes = new Uint8Array(await request.blob.arrayBuffer());
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
    } else {
      reply = wasm_bindgen.structure_check(
        bytes,
        password,
        Boolean(request.attemptRecovery),
        limits,
      );
    }
  } catch {
    // A trap, or an Emscripten abort. Deliberately no binding: the text is never inspected,
    // logged or forwarded. This instance is poisoned (ADR 0009), so the reply is fatal and
    // the page terminates the worker.
    self.postMessage(internalFailure(request.id, "internal error"));
    return;
  }

  // GUARDED, and it was not until M1 PR 4a-ii found out the hard way.
  //
  // `drainReply` reads getters off a wasm-bindgen object. A stale `pkg/` -- a Rust field added
  // and the module not rebuilt -- makes one of them `undefined`, `.toString()` throws, the
  // throw escapes this handler, and NO REPLY IS EVER SENT. The symptom was every operation
  // failing as `max_duration_ms exceeded` thirty seconds later: the watchdog doing its job,
  // reporting the only thing it can see, and naming entirely the wrong cause.
  //
  // Turning that into an immediate typed `Internal` costs three lines and is the difference
  // between a confusing half-minute and an obvious failure.
  try {
    self.postMessage(drainReply(request.id, reply));
  } catch {
    self.postMessage(internalFailure(request.id, "internal error"));
  }
};
