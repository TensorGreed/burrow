// The render bundle's engine and dispatch: PDFium. Last file in `burrow-render-worker.js`.
//
// THE PROTOCOL IS NOT HERE. `worker-protocol.js` owns the policy guard, the memoised init
// promise, the ack, the reply flattening and every refusal shape, and it is the same file
// this bundle and the base one both carry. What this file owes it is the same three names
// `main.js` owes it — `init`, `KNOWN_OPS` and `runOperation` — which is the whole of what the
// two bundles disagree about.
//
// WHAT THIS BUNDLE ANSWERS
//
// `page_count` and `render`. It answered only the first for one merge, deliberately: ADR 0026
// shipped the loading boundary with the smallest honest payload behind it, because a boundary
// with nothing behind it is one no browser test can drive, and an untested worker whose guard
// decides whether file bytes may be touched is exactly the thing that goes wrong quietly.
//
// `render` is #57's second piece, and it arrived with its ceiling rather than before it:
// ADR 0027 is the record, and ADR 0020 is why the ceiling had to be designed rather than
// discovered under pressure from a UI.

async function init() {
  // PDFium's glue is NOT modularised: it begins instantiating as the bundle is parsed, reading
  // the `self.Module` that `render-prelude.js` assigned before it. So readiness is a callback
  // on that global rather than the `await createQpdfModule(...)` the base bundle uses, and
  // there is nothing to call here to start it.
  //
  // THAT ASYMMETRY IS ALSO THE SAFETY PROPERTY. `pdfium.js` keeps its state in worker-global
  // `var`s, so a second evaluation would rebind every glue global while the bridge still held
  // the first instance — spike 0001's HIGH finding, where calls read and write the other
  // instance's linear memory and parses come back confidently wrong. There is no second
  // evaluation to make: the glue is in the bundle, and a worker scope evaluates it once. ADR
  // 0006's amendment records this as closed *by construction* for PDFium, as opposed to
  // closed by the memoised promise for qpdf.
  // `pdfiumReady` IS CREATED IN `render-prelude.js`, NOT HERE, and that is the fix for a
  // measured hang rather than a style choice: `onRuntimeInitialized` is called once,
  // optionally, and a callback assigned at this point loses the race to a warm HTTP cache.
  // `render-prelude.js`'s own comment has the measurement.
  //
  // BELT AND BRACES. If the runtime came up before this line — which is the common case, and
  // the whole reason the callback had to be registered earlier — the promise has already
  // resolved and this is a no-op. If a future edit ever loses the callback again, this makes
  // the failure a working worker rather than a four-minute hang; it is not a substitute for
  // the callback, because it only runs when something asks.
  if (self.Module.calledRun) {
    // The same narrowing `render-prelude.js` makes, guarded by the same fact: `calledRun` is
    // set inside `doRun` after `initRuntime()`, so the exports are on the object.
    resolvePdfium(/** @type {EmscriptenModule} */ (/** @type {unknown} */ (self.Module)));
  }
  const pdfiumModule = await pdfiumReady;

  // PDFium's library init is global and must happen exactly once. On native, two threads
  // calling it concurrently abort the process with `SIGTRAP` (`core/CLAUDE.md`); a worker has
  // one thread, so what is left here is only the "exactly once" half, which the memoised init
  // promise in `worker-protocol.js` already guarantees.
  pdfiumModule._FPDF_InitLibrary();

  __burrow_attach(pdfiumModule);

  // The Rust module last: its imports are the `__burrow_pdfium_*` globals, which now exist.
  // The compiled module is handed in rather than a URL, so every `.wasm` this worker loads
  // goes through the same integrity-checked fetch.
  const burrowModule = await compiled["burrowRenderWasm"];
  await wasm_bindgen({ module_or_path: burrowModule });
}

/**
 * The operations this bundle answers.
 *
 * `page_count` is PDFium's, not qpdf's, and the two bundles giving the same name to two
 * engines' answers is deliberate rather than an oversight: they never appear together, a page
 * that has the render worker open has it because it wanted PDFium, and naming this
 * `pdfium_page_count` would put the engine in the protocol — which is the one place ADR 0009
 * keeps engine choice out of.
 *
 * **No conformance case compares the two.** The corpus asks one question per operation per
 * platform, and a second web answer for `page_count` would mean deciding what a disagreement
 * between two *web* engines means. That is a real question — spike 0004 measured five
 * divergences between these exact two readings — and it is not this change's question.
 */
const KNOWN_OPS = new Set(["page_count", "render"]);

/**
 * Draw a strip, posting each page as it is produced.
 *
 * ADR 0023's protocol, reached for ADR 0027 §2's reason: the worker pulls one page, posts it,
 * and drops it, so the engine heap holds **at most one bitmap** however long the strip is and
 * the main thread never receives the whole set at once.
 *
 *   `{ id, progress: { part, of } }`                             the total, before anything
 *   `{ id, page: { number, index, of, width, height }, pixels }` per page
 *   the ordinary terminal reply, carrying no pixels
 *
 * A consumer that reads the terminal reply and ignores the pages gets nothing rather than the
 * first page, which is the direction to fail in.
 *
 * ONE DIFFERENCE FROM `split`, AND IT IS THE OPPOSITE RULE. A failure part way through does
 * **not** invalidate the pages already delivered: a split is a partition, so a subset of the
 * parts is not a partition of anything, but a strip is a set of independent pictures and the
 * ones that arrived are still true pictures of their pages. The host may keep them and show
 * the gap.
 *
 * THE PIXELS ARE TRANSFERRED, not copied. `take_pixels()` has already moved them out of the
 * wasm heap into a `Uint8Array`; posting its buffer in the transfer list hands that same
 * allocation to the page and detaches it here. A thumbnail is 300 KB and a strip is dozens of
 * them, so a structured-clone copy of each would be the largest avoidable cost on this path.
 *
 * WHY NOT A BLOB, WHICH IS WHAT `split` POSTS. A Blob is right for a document: the page turns
 * it into a download and never wants the bytes. These bytes ARE wanted -- the page draws them
 * into a canvas -- so a Blob would have to be read back asynchronously into an ArrayBuffer,
 * which is the copy this avoids. `img-src 'self'` admits no `blob:` either, so there was never
 * a URL to be had (ADR 0014).
 *
 * @param {number} id
 * @param {Uint8Array} bytes
 * @param {Uint32Array} pages
 * @param {number} boxWidth
 * @param {number} boxHeight
 * @param {Uint8Array | undefined} password
 * @param {WebLimits} limits
 */
function renderStrip(id, bytes, pages, boxWidth, boxHeight, password, limits) {
  let session;
  try {
    session = wasm_bindgen.render_begin(bytes, pages, boxWidth, boxHeight, password, limits);
  } catch {
    self.postMessage(internalFailure(id, "internal error"));
    return;
  }
  try {
    if (!session.ok) {
      self.postMessage(drainReply(id, session.begin_reply()));
      return;
    }
    const of = session.pages;
    // THE TOTAL FIRST, before any page. A strip needs to know how many tiles to lay out, and
    // a consumer that joined late needs it too -- which is why every page carries `of` as well.
    self.postMessage({ id, progress: { part: 0, of } });

    for (let index = 0; index < of; index += 1) {
      const drawn = session.next_page();
      try {
        let flat;
        try {
          flat = drainReply(id, drawn.reply());
        } catch {
          self.postMessage(internalFailure(id, "internal error"));
          return;
        }
        if (!flat.ok) {
          // The strip ends here. The failure IS the terminal reply, so a consumer that only
          // watches for one still learns about it -- and the pages already posted stand.
          self.postMessage(flat);
          return;
        }

        // `pixelLength` is read before taking, because both "no pixels" and "already taken"
        // come back as an empty array and only the length can tell them apart. Nothing here
        // decides anything from it: it is the same read `drainReply` makes of `outputLength`.
        const pixels = drawn.pixelLength > 0 ? drawn.takePixels() : new Uint8Array(0);
        self.postMessage(
          {
            id,
            page: {
              number: Number(drawn.page),
              index,
              of,
              width: drawn.width,
              height: drawn.height,
            },
            pixels: pixels.buffer,
          },
          [pixels.buffer],
        );
        self.postMessage({ id, progress: { part: index + 1, of } });
      } finally {
        // A wasm-bindgen object is a boxed Rust value in a heap that never shrinks. One leaked
        // page per tile is a worker that grows with the length of the strip.
        drawn.free();
      }
    }

    // The terminal reply: success, no pixels.
    self.postMessage(refusalFree(id));
  } finally {
    session.free();
  }
}

/**
 * Run one render-bundle operation and hand back its reply.
 *
 * Called by `worker-protocol.js` after the policy guard, after `ensureReady()`, after the
 * unknown-operation refusal and after the ack — so everything here is time attributable to
 * the file, which is what the page's watchdog is measuring.
 *
 * @param {Record<string, any>} request
 * @returns {Promise<Reply | null>}
 */
async function runOperation(request) {
  // THE BUDGET IS CHECKED BEFORE A SINGLE BYTE IS READ, exactly as `main.js` does it, and the
  // ordering is the whole point of the call.
  //
  // THIS BUNDLE USED TO SKIP IT, on the argument that it "takes ONE blob and copies it once,
  // so the ceiling `WebPdfium::open` applies is reached before anything of comparable size has
  // been allocated". Code review measured that and it is wrong twice: `main.js` runs the
  // pre-flight for every operation rather than only for `merge`, so the two bundles disagreed
  // about a ceiling; and the read below materialises the whole file in this heap and
  // wasm-bindgen copies it again into linear memory, both BEFORE `open` applies
  // `max_input_bytes` — so an over-ceiling file cost 2x its size before the typed refusal it
  // was supposed to get instead.
  //
  // A `Blob`'s `size` is already known; nothing is read to ask this question. And the question
  // is asked in RUST, by the same function the operation itself calls — ADR 0009 §2 forbids a
  // binding enforcing any part of `Limits`, and comparing these numbers here would be that.
  const budget = wasm_bindgen.check_input_budget(
    Float64Array.from([request.blob.size]),
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

  // A Blob, not a transferred ArrayBuffer, for the reason `main.js` gives at length: structured
  // clone passes a Blob BY REFERENCE, so the page never materialises the bytes in its own heap
  // and still holds a usable handle to the same file after a worker is killed.
  //
  // Reading a Blob is a memory read. It issues no request, so no CSP directive is consulted.
  const bytes = new Uint8Array(await request.blob.arrayBuffer());
  const password = request.password ? new Uint8Array(request.password) : undefined;

  // NOT freed here. wasm-bindgen passes a struct argument BY VALUE: the generated glue calls
  // `__destroy_into_raw()` and hands the raw pointer to Rust, which then owns it. Calling
  // `.free()` afterwards is a double free and throws "null pointer passed to rust", which
  // manifests as the worker never replying at all.
  const limits = new wasm_bindgen.WebLimits(
    BigInt(request.limits.maxInputBytes),
    BigInt(request.limits.maxMemoryBytes),
    BigInt(request.limits.maxDurationMs),
    BigInt(request.limits.maxPages),
    BigInt(request.limits.maxPixels),
  );

  if (request.op === "render") {
    // Every number that shapes the request comes from the page and is validated in RUST: the
    // page list against the real page count, the box against `max_pixels`, both inside
    // `render_begin`. Nothing here compares anything (ADR 0009 §2).
    renderStrip(
      request.id,
      bytes,
      Uint32Array.from(request.pages),
      request.boxWidth,
      request.boxHeight,
      password,
      limits,
    );
    return null;
  }

  return wasm_bindgen.page_count(bytes, password, limits);
}
