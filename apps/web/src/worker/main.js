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

  // NO PDFIUM STEP. It was three things here: awaiting `Module.onRuntimeInitialized` for glue
  // that began instantiating when the bundle was parsed, a narrowing cast from config to
  // module, and a library-init call. All of it went with the artifact in spike 0004.
  //
  // qpdf needs none of them: `createQpdfModule()` returns a promise for an initialised module,
  // so readiness is the `await` above rather than a callback on a global.

  __burrow_attach(qpdfModule);

  // The Rust module last: its imports are the `__burrow_*` globals, which now exist. The
  // compiled module is handed in rather than a URL, so every `.wasm` the worker loads goes
  // through the same integrity-checked fetch.
  const burrowModule = await compiled["burrowWasm"];
  await wasm_bindgen({ module_or_path: burrowModule });
}

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
    qpdfHeapBytes: "0",
    rotations: [],
    failedInput: -1,
    innerKind: "",
  };
}

/**
 * Run a split, posting each part as it is produced.
 *
 * ADR 0023's protocol, and the only place in this worker that posts more than one message for
 * one request:
 *
 *   `{ id, part: { index, of }, output }`  per part, then the ordinary terminal reply.
 *
 * The terminal reply carries **no bytes**. A consumer that reads `reply.output` and ignores the
 * parts gets nothing rather than the first part, which is the direction to fail in.
 *
 * Progress is per part and that is all the core can honestly report: there is no progress hook
 * inside an operation, so a bar that interpolated within a part would be inventing a number.
 *
 * @param {number} id
 * @param {Uint8Array} bytes
 * @param {Uint32Array} cuts
 * @param {Uint8Array | undefined} password
 * @param {WebLimits} limits
 */
function splitInto(id, bytes, cuts, password, limits) {
  let session;
  try {
    session = wasm_bindgen.split_begin(bytes, cuts, password, limits);
  } catch {
    self.postMessage(internalFailure(id, "internal error"));
    return;
  }
  try {
    if (!session.ok) {
      self.postMessage(drainReply(id, session.begin_reply()));
      return;
    }
    const of = session.parts;
    // THE TOTAL FIRST, before any part. A progress bar needs to know how many are coming, and
    // a consumer that joined late needs it too -- which is why every part carries `of` as well.
    self.postMessage({ id, progress: { part: 0, of } });

    for (let index = 0; index < of; index += 1) {
      const reply = session.next_part();
      let flat;
      try {
        flat = drainReply(id, reply);
      } catch {
        self.postMessage(internalFailure(id, "internal error"));
        return;
      }
      if (!flat.ok) {
        // THE WHOLE SPLIT FAILS. Nothing further is posted and the host discards the parts it
        // already holds -- ADR 0023 §3. The failure is the terminal reply, so a consumer that
        // only watches for one still learns about it.
        self.postMessage(flat);
        return;
      }
      // A BLOB, exactly as a single-output reply carries one. Structured clone passes it BY
      // REFERENCE, so the part never lands in the main thread's own heap -- and `drainReply`
      // has already taken it out of the wasm heap, which is what makes pulling one at a time
      // worth doing. No transfer list: a Blob is not transferable and does not need to be.
      self.postMessage({ id, part: { index, of }, output: flat.output });
      self.postMessage({ id, progress: { part: index + 1, of } });
    }

    // The terminal reply: success, no bytes.
    self.postMessage(refusalFree(id));
  } finally {
    session.free();
  }
}

/**
 * A terminal success for a multi-output operation: every part already went, so this carries none.
 *
 * @param {number} id
 */
function refusalFree(id) {
  return { ...refusal(id, "", ""), ok: true, message: "" };
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
      qpdfHeapBytes: reply.qpdf_heap_bytes.toString(),
      // EVERY PAGE'S ROTATION, for `page_rotations`. Numbers rather than the strings
      // `requested` and `allowed` use: a rotation is 0, 90, 180 or 270, so there is no `u64`
      // here to round. The getter hands back a BigInt64Array because `/Rotate` is an integer
      // in the file and the type that reads it is `i64`; the conversion is the only
      // arithmetic on this line and it cannot lose a quarter turn.
      rotations: Array.from(reply.rotations, (n) => Number(n)),
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

    if (
      request.op !== "page_count" &&
      request.op !== "structure_check" &&
      request.op !== "merge" &&
      request.op !== "rotate" &&
      request.op !== "reorder" &&
      request.op !== "split" &&
      request.op !== "page_rotations"
    ) {
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
        qpdfHeapBytes: "0",
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
    // ONE BLOB OR MANY. `merge` is the first operation with more than one input, and the
    // Blob discipline above applies to each of them: the page holds a list of `File`s and
    // hands them over by reference, so it never materialises any of the bytes and still
    // holds every handle after a worker is killed.
    //
    // `blobs` and `blob` are kept distinct rather than unified into a one-element list,
    // because `run()`'s contract is per-operation and a single-input op that suddenly took
    // a list would be a silent change to every existing caller.
    const inputs = request.op === "merge" ? request.blobs : [request.blob];

    // THE BUDGET IS CHECKED BEFORE A SINGLE BYTE IS READ, and that ordering is the whole
    // point of this call.
    //
    // `burrow_core::ops::merge` checks the aggregate `max_input_bytes` before it opens any
    // document, which is the right place in the core and is too late here: the loop below
    // materialises the payload once, the flat buffer copies it again, and wasm-bindgen
    // copies that into linear memory -- so a selection several times over the ceiling can
    // exhaust the tab on the way in, and the person sees "something inside burrow failed"
    // instead of the refusal the ceiling exists to give them. Issue #51.
    //
    // A `Blob`'s `size` is already known; nothing is read to ask this question. And the
    // question is asked in RUST, by the same function `merge` itself calls -- ADR 0009 §2
    // forbids a binding enforcing any part of `Limits`, and comparing these numbers here
    // would be exactly that.
    const budget = wasm_bindgen.check_input_budget(
      Float64Array.from(inputs, (blob) => blob.size),
      new wasm_bindgen.WebLimits(
        BigInt(request.limits.maxInputBytes),
        BigInt(request.limits.maxMemoryBytes),
        BigInt(request.limits.maxDurationMs),
        BigInt(request.limits.maxPages),
        BigInt(request.limits.maxPixels),
      ),
    );
    const verdict = drainReply(request.id, budget);
    if (!verdict.ok) {
      self.postMessage(verdict);
      return;
    }

    const buffers = [];
    for (const blob of inputs) {
      buffers.push(new Uint8Array(await blob.arrayBuffer()));
    }
    const bytes = buffers[0];
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

    if (request.op === "merge") {
      // ONE FLAT BUFFER PLUS A LENGTH TABLE, not an array of arrays. wasm-bindgen can
      // marshal `Vec<Vec<u8>>` and doing so copies every document twice. A merge is the
      // largest thing this boundary carries, and the page's whole Blob discipline exists so
      // the bytes live in as few places as possible; undoing that at the last step would be
      // perverse. Rust validates the table against the buffer rather than trusting it.
      let total = 0;
      for (const b of buffers) total += b.length;
      const flat = new Uint8Array(total);
      const lengths = new Uint32Array(buffers.length);
      let at = 0;
      for (let i = 0; i < buffers.length; i += 1) {
        flat.set(buffers[i], at);
        lengths[i] = buffers[i].length;
        at += buffers[i].length;
      }
      // Drop the per-input copies before the call, so the peak is the flat buffer plus the
      // engine heap rather than both plus the originals.
      buffers.length = 0;
      reply = wasm_bindgen.merge(flat, lengths, limits);
    } else if (request.op === "rotate") {
      // ONE INPUT, ONE OUTPUT, and no new reply shape: `merge` made `Reply` carry bytes and
      // rotate needs nothing more. The page list and the angle are the only additions, and
      // both are numbers the page chose -- nothing here is derived from the document.
      //
      // `Uint32Array` rather than an array of numbers, for the same reason merge sends a
      // flat buffer: wasm-bindgen marshals a typed array as one copy.
      // REFUSED, NOT COERCED. `Uint32Array.from` wraps a number above 2^32 and floors a
      // fractional one, so a malformed request would rotate a DIFFERENT page and report
      // success -- upstream of the core's careful refusal of page 0 ("a caller that is off by
      // one should hear about it"). These are caller arguments rather than file content, so
      // this is not the attacker-controlled-size rule; it is the same principle one layer out.
      // Found by code review.
      // AND THE ANGLE, for the same reason. wasm-bindgen coerces a JS number to `i32` by
      // truncation, so a fractional or out-of-range angle becomes a DIFFERENT angle rather
      // than a refusal -- the asymmetry the comment below argues against, one argument over.
      // The island can only produce 90, 180 or 270 from radio buttons, so nothing on this
      // page reaches it; the worker is not the island's private API. Found by security review.
      const degrees = request.degrees ?? 0;
      const angleUsable =
        typeof degrees === "number" && Number.isInteger(degrees) && degrees % 90 === 0;

      const pages = request.pages ?? [];
      const usable = pages.every(
        /** @param {unknown} n */
        (n) => typeof n === "number" && Number.isInteger(n) && n >= 1 && n <= 0xffff_ffff,
      );
      if (!usable || !angleUsable) {
        self.postMessage({
          id: request.id,
          ok: false,
          kind: "InvalidArgument",
          fatal: false,
          message: usable
            ? "a rotation must be a whole multiple of 90 degrees"
            : "a page number is not a whole number in range",
          pages: 0,
          limit: "",
          stage: "",
          requested: "0",
          allowed: "0",
          recycle: false,
          qpdfHeapBytes: "0",
          rotations: [],
          failedInput: -1,
          innerKind: "",
        });
        return;
      }
      reply = wasm_bindgen.rotate(bytes, Uint32Array.from(pages), degrees, password, limits);
    } else if (request.op === "reorder") {
      // ONE INPUT, ONE OUTPUT, and the same `Uint32Array` marshalling rotate uses. The order
      // is the only addition and it is entirely the page's: a permutation of page numbers,
      // nothing derived from the document.
      //
      // REFUSED, NOT COERCED, for the reason rotate's page list is: `Uint32Array.from` wraps
      // a number above 2^32 and floors a fractional one, so a malformed request would produce
      // a DIFFERENT permutation and report success. Here that is worse than for rotate --
      // a wrapped number is still a valid page index, so the result is a correctly-formed
      // document with its pages in an order nobody asked for.
      const order = request.order ?? [];
      const orderUsable =
        Array.isArray(order) &&
        order.every(
          /** @param {unknown} n */
          (n) => typeof n === "number" && Number.isInteger(n) && n >= 1 && n <= 0xffff_ffff,
        );
      if (!orderUsable) {
        self.postMessage({
          id: request.id,
          ok: false,
          kind: "InvalidArgument",
          fatal: false,
          message: "a page number is not a whole number in range",
          pages: 0,
          limit: "",
          stage: "",
          requested: "0",
          allowed: "0",
          recycle: false,
          qpdfHeapBytes: "0",
          rotations: [],
          failedInput: -1,
          innerKind: "",
        });
        return;
      }
      reply = wasm_bindgen.reorder(bytes, Uint32Array.from(order), password, limits);
    } else if (request.op === "split") {
      // THE ONLY MULTI-OUTPUT OPERATION, and the only arm that posts more than one message.
      // ADR 0023: parts stream out one at a time so the engine heap holds one rather than all
      // of them, each is verified before it is posted, and a failure anywhere fails the whole
      // split -- the host discards what it has, because a subset of the parts is not a
      // partition of anything.
      //
      // REFUSED, NOT COERCED, exactly as reorder's order is. A cut is a page number the page
      // chose; `Uint32Array.from` wraps above 2^32 and floors a fraction, so a malformed
      // request would silently partition the document somewhere else and report success.
      const cuts = request.cuts ?? [];
      const cutsUsable =
        Array.isArray(cuts) &&
        cuts.every(
          /** @param {unknown} n */
          (n) => typeof n === "number" && Number.isInteger(n) && n >= 1 && n <= 0xffff_ffff,
        );
      if (!cutsUsable) {
        self.postMessage(
          refusal(request.id, "InvalidArgument", "a cut is not a whole page number in range"),
        );
        return;
      }
      splitInto(request.id, bytes, Uint32Array.from(cuts), password, limits);
      return;
    } else if (request.op === "page_rotations") {
      reply = wasm_bindgen.page_rotations(bytes, password, limits);
    } else if (request.op === "page_count") {
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
