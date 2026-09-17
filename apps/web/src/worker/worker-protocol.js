// The worker's message protocol, shared by both bundles.
//
// WHAT IS HERE AND WHAT IS NOT
//
// Everything that is true of a request whatever engine answers it: the fail-closed policy
// guard, the memoised init promise, the ack that starts the page's watchdog, the reply
// flattening, and the refusal shapes. What is NOT here is which engine to start and which
// operations exist -- those are the only two things the two bundles disagree about.
//
// THE CONTRACT WITH THE FILE AFTER THIS ONE. The bundle's last file supplies three names:
//
//   async function init()                     start this bundle's engine and attach the bridge
//   const KNOWN_OPS = new Set([...])          the operations this bundle answers
//   async function runOperation(request)      -> a `Reply` to post, or null if it posted itself
//
// They are read, never written, from here. Function declarations hoist across the whole
// concatenated bundle, which is the same mechanism by which `main.js` already reaches
// `instantiateFrom` and `POLICED` from `prelude.js` and `__burrow_attach` from the bridge.
//
// WHY ONE IMPLEMENTATION RATHER THAN TWO MAINS. A second worker written by copying this one
// would start without the fixes this one accumulated: the memoised **promise** rather than a
// boolean (spike 0001's HIGH finding), the ack placed after start-up and before the file is
// read (the queue-time bug PR 2 fixed natively), the guarded `drainReply` that turns a stale
// `pkg/` into an immediate typed failure rather than a watchdog kill thirty seconds later,
// and the refusal shapes that carry every field a reader looks for. Each of those was a
// measured failure, and a copy is where they would be missing.
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
// completion, and the second load rebound every glue global while the bridge still held the
// first instance. Calls then read and wrote the other instance's linear memory — the sandbox
// holds, so parses simply come back confidently wrong, which is worse.
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

function ensureReady() {
  // `??=` assigns the promise, not its result, and only when there is not one already.
  //
  // IT MEMOISES REJECTION TOO, DELIBERATELY. If `init()` throws -- a trap in
  // a library-init call, a failed engine fetch -- `ready` stays a rejected promise for the
  // worker's life and every later operation fails `Internal`/fatal. That is fail-closed and
  // it is the intended behaviour. Do not "fix" it by clearing `ready` on failure: a retry
  // would call `createQpdfModule()` a second time and rebind the bridge's module while an
  // in-flight call still held the first instance, which is spike 0001's HIGH finding
  // reintroduced. `init-memoisation.test.ts` would NOT catch that edit -- its mutation is the
  // boolean-flag revert -- so this comment is the only thing standing in front of it.
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
    engineHeapBytes: "0",
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
 * A typed refusal the worker itself decided, in the shape `drainReply` produces.
 *
 * Extracted because the split arm needs one too, and a second hand-written literal is a second
 * place for a field to go missing -- which `drainReply`'s own comment records as costing a
 * confusing half-minute when a getter came back `undefined`.
 *
 * @param {number} id
 * @param {string} kind
 * @param {string} message
 */
function refusal(id, kind, message) {
  return {
    id,
    ok: false,
    kind,
    fatal: false,
    message,
    pages: 0,
    limit: "",
    stage: "",
    requested: "0",
    allowed: "0",
    recycle: false,
    engineHeapBytes: "0",
    rotations: [],
    failedInput: -1,
    innerKind: "",
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
      engineHeapBytes: reply.engine_heap_bytes.toString(),
      // EVERY PAGE'S ROTATION, for `page_rotations`. Numbers rather than the strings
      // `requested` and `allowed` use: a rotation is 0, 90, 180 or 270, so there is no `u64`
      // here to round. The getter hands back a BigInt64Array because `/Rotate` is an integer
      // in the file and the type that reads it is `i64`; the conversion is the only
      // arithmetic on this line and it cannot lose a quarter turn.
      rotations: Array.from(reply.rotations, (n) => Number(n)),
      // BOTH SIZES A COMPARISON WAS DECIDED ON, for `compress`. Strings for the same reason
      // `requested` and `allowed` are: these are `u64`, and a document above 2^53 bytes is
      // not reachable but the type is the type -- rounding a size a person is reading would
      // be a small lie with no upside.
      //
      // They ride on every reply so a page needs no second round trip: `compress` is the one
      // operation whose successful answer can be "nothing, and here is why", and
      // `producedBytes` is the ONLY record of what the discarded re-encoding weighed. Zero
      // for every other operation.
      originalBytes: reply.originalBytes.toString(),
      producedBytes: reply.producedBytes.toString(),
      // WHICH input failed, as a number rather than something to parse out of `message`.
      // -1 when the failure is not about a particular input.
      failedInput: reply.failedInput,
      // What is wrong with that input, so a page need not unwrap anything itself.
      innerKind: reply.innerKind,
      // THE DOCUMENT, AS A BLOB, AND THE BLOB IS THE POINT.
      //
      // `takeOutput()` MOVES the bytes out of the Rust reply -- a getter would copy them,
      // and this is the largest thing the boundary carries. Wrapping them in a Blob here
      // rather than posting the Uint8Array means structured clone passes them to the page
      // BY REFERENCE, exactly as the page passes inputs in: the main thread never holds a
      // merged document in its own heap, it holds a handle it can turn into a download.
      //
      // `outputLength` is read first because both "no document" and "already taken" come
      // back as an empty array, and only the length can tell them apart.
      output:
        reply.outputLength > 0 ? new Blob([reply.takeOutput()], { type: "application/pdf" }) : null,
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

    // THE OPERATION LIST IS THE BUNDLE'S, and the refusal is the protocol's. Each bundle
    // answers a different set, so the list lives with the dispatch that implements it while
    // the shape of the refusal stays identical either way.
    if (!KNOWN_OPS.has(request.op)) {
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
        engineHeapBytes: "0",
      });
      return;
    }

    // THE ACK, AND WHY IT IS HERE RATHER THAN AT THE TOP OF THIS HANDLER.
    //
    // The page's watchdog starts its clock on this message, not on its own `postMessage`.
    // Everything above this line -- the policy guard, and a cold engine compile that
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

    // THE BUNDLE'S OWN DISPATCH. It returns a `Reply` to be flattened and posted, or `null`
    // when it has already posted everything itself -- which is `split` and ADR 0023's
    // multi-message protocol, where the terminal message carries no bytes and the parts went
    // before it.
    reply = await runOperation(request);
    if (reply === null) {
      return;
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
