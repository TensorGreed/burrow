// The base bundle's engine and dispatch: qpdf, and the seven operations that produce or read
// a document. Last file in `burrow-worker.js`; everything it uses exists by now.
//
// THE PROTOCOL IS NOT HERE. `worker-protocol.js` owns the policy guard, the memoised init
// promise, the ack, the reply flattening and every refusal shape, and it is byte-identical in
// the render bundle. What this file owes it is exactly three names — `init`, `KNOWN_OPS` and
// `runOperation` — which is the whole of what the two bundles disagree about.
//
// The rules that used to be stated here (never echo what the module threw; memoise the
// promise and not a boolean; the ack goes after start-up and before the file is read) are
// stated there instead, beside the code that implements them, rather than duplicated beside
// the code that does not.

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

/**
 * The operations this bundle answers.
 *
 * A `Set` rather than the chain of `!==` this was: two bundles need the same refusal against
 * two different lists, so the list became data. The refusal itself is in
 * `worker-protocol.js`, and it still happens BEFORE a `WebLimits` is constructed — an unknown
 * op is a bug in the page rather than a poisoned engine, and building a limits struct on a
 * path that never consumes it leaks a boxed Rust value into a heap that never shrinks.
 */
const KNOWN_OPS = new Set([
  "page_count",
  "structure_check",
  "merge",
  "rotate",
  "compress",
  "reorder",
  "split",
  "page_rotations",
]);

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
 * Run one document operation and hand back its reply.
 *
 * Called by `worker-protocol.js` after the policy guard, after `ensureReady()`, after the
 * unknown-operation refusal and after the ack — so everything here is time attributable to
 * the file, which is what the page's watchdog is measuring.
 *
 * Returns the `Reply` for the protocol to flatten and post, or **null** when it has already
 * posted everything itself: a refusal it could phrase better than a `Reply` could, or
 * `split`, whose parts went one at a time under ADR 0023 and whose terminal message carries
 * no bytes.
 *
 * @param {Record<string, any>} request
 * @returns {Promise<Reply | null>}
 */
async function runOperation(request) {
  let reply = null;

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
    return null;
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
        engineHeapBytes: "0",
        rotations: [],
        failedInput: -1,
        innerKind: "",
      });
      return null;
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
        engineHeapBytes: "0",
        rotations: [],
        failedInput: -1,
        innerKind: "",
      });
      return null;
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
      return null;
    }
    splitInto(request.id, bytes, Uint32Array.from(cuts), password, limits);
    return null;
  } else if (request.op === "compress") {
    // ONE INPUT, AND SOMETIMES NO OUTPUT. Compression takes no selection at all -- no page
    // list, no angle, no cut -- so there is nothing here to validate or refuse: the whole
    // request is the document.
    //
    // The reply carries `originalBytes` and `producedBytes` whichever way it went, so a
    // page can report "already efficiently stored" from this one message. An empty output
    // on a successful reply is NOT an error here; see the reply shape below.
    reply = wasm_bindgen.compress(bytes, password, limits);
  } else if (request.op === "page_rotations") {
    reply = wasm_bindgen.page_rotations(bytes, password, limits);
  } else if (request.op === "page_count") {
    reply = wasm_bindgen.page_count(bytes, password, limits);
  } else {
    reply = wasm_bindgen.structure_check(bytes, password, Boolean(request.attemptRecovery), limits);
  }

  return reply;
}
