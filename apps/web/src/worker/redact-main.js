// The redaction bundle's engine and dispatch: qpdf, and one operation. Last file in
// `burrow-redact-worker.js` (#137).
//
// THE PROTOCOL IS NOT HERE. `worker-protocol.js` owns the policy guard, the memoised init
// promise, the ack, the reply flattening and every refusal shape, and it is byte-identical in
// all three bundles. What this file owes it is the same three names `main.js` and
// `render-main.js` owe it -- `init`, `KNOWN_OPS` and `runOperation`.
//
// WHY A THIRD BUNDLE AT ALL. ADR 0029's 2026-09-21 amendment: redaction's Rust is 164,616
// brotli bytes, nearly three times the base module, and an entry point in the base module
// would put all of it on every tool page's first load. So it has its own module and its own
// bundle, fetched on the first redaction and by nothing else.
//
// STAGED INTO HARNESS BUILDS ONLY, for now. `/redact-pdf` is held under #125 and #180-#183, and
// `src/production-build.test.ts` holds the op name `redact` out of every shipped script. So
// `tools/stage-web-engines.mjs` stages this bundle only when `BURROW_HARNESS=1`, which is what
// #137's R8/R9 specs and the browser differential drive.
//
// R8 AND R9 ARE THIS FILE'S TO KEEP (ADR 0006): the output is one value, posted once, after
// the Rust call has returned success and verified it; nothing leaves the heap before that --
// no chunk, no `blob:` URL, no file handle. The reply is built by `drainReply`, which makes a
// Blob only of what `takeOutput()` hands back on a successful reply.

async function init() {
  // The same start-up as the base bundle: qpdf is MODULARIZE'd, so readiness is an `await`
  // rather than a callback on a global, and the memoised init promise in `worker-protocol.js`
  // is what makes a second instantiation impossible (ADR 0006's amendment).
  const qpdfModule = await createQpdfModule({
    ...silent,
    instantiateWasm: instantiateFrom("qpdfWasm"),
  });

  __burrow_attach(qpdfModule);

  // Redaction's own Rust module, last: its imports are the `__burrow_*` globals, which exist
  // now. Handed in compiled, so every `.wasm` this worker loads went through the same
  // integrity-checked fetch.
  const burrowModule = await compiled["burrowRedactWasm"];
  await wasm_bindgen({ module_or_path: burrowModule });
}

/** The one operation this bundle answers. */
const KNOWN_OPS = new Set(["redact"]);

/**
 * A 1-based page number, as the core takes it: a whole number from 1 to 2^32 - 1.
 *
 * REFUSED, NOT COERCED, for `rotate`'s reason: wasm-bindgen converts a JS number to `u32` by
 * truncating, so a malformed page would redact a DIFFERENT page and report success. These are
 * caller arguments rather than file content; zero is refused by Rust, which says why.
 *
 * @param {unknown} n
 */
function pageNumber(n) {
  return typeof n === "number" && Number.isInteger(n) && n >= 0 && n <= 0xffff_ffff;
}

/**
 * Refuse a request whose arguments cannot be handed to Rust as the caller meant them.
 *
 * @param {number} id
 * @param {string} message
 */
function badArguments(id, message) {
  self.postMessage(refusal(id, "InvalidArgument", message));
  return null;
}

/**
 * Run one redaction and hand back its reply, or **null** when a refusal was already posted.
 *
 * @param {Record<string, any>} request
 * @returns {Promise<Reply | null>}
 */
async function runOperation(request) {
  // THE ARGUMENTS FIRST, BEFORE A BYTE IS READ. Everything here is a number the page chose;
  // nothing is derived from the document, and a malformed request is a bug in the page rather
  // than a reason to touch the file.
  const page = request.page;
  const covered = request.covered ?? [];
  const region = request.region;
  if (!pageNumber(page) || !Array.isArray(covered) || !covered.every(pageNumber)) {
    return badArguments(request.id, "a page number is not a whole number in range");
  }
  // NUMBERS, NOT NECESSARILY FINITE ONES. wasm-bindgen passes a JS number to an `f64` exactly,
  // and a string would arrive as NaN with nobody told -- so the TYPE is checked here. Whether a
  // finite, non-empty region is inside the page is the core's question, and it refuses by name.
  const sides = region ? [region.left, region.top, region.width, region.height] : [];
  if (sides.length !== 4 || !sides.every((/** @type {unknown} */ n) => typeof n === "number")) {
    return badArguments(request.id, "a region is four numbers: left, top, width and height");
  }

  // THE BUDGET BEFORE A SINGLE BYTE IS READ, and in Rust -- the base bundle's reason (#51).
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

  const bytes = new Uint8Array(await request.blob.arrayBuffer());
  const password = request.password ? new Uint8Array(request.password) : undefined;
  // BOTH STRUCTS ARE CONSUMED BY THE CALL: wasm-bindgen moves them into Rust. Freeing either
  // afterwards is a double free; `main.js` records what that looks like.
  const limits = new wasm_bindgen.WebLimits(
    BigInt(request.limits.maxInputBytes),
    BigInt(request.limits.maxMemoryBytes),
    BigInt(request.limits.maxDurationMs),
    BigInt(request.limits.maxPages),
    BigInt(request.limits.maxPixels),
  );
  const area = new wasm_bindgen.WebRegion(region.left, region.top, region.width, region.height);

  return wasm_bindgen.redact(bytes, page, Uint32Array.from(covered), area, password, limits);
}
