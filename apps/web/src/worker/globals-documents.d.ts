// The base bundle's half of the worker's ambient declarations: what `burrow-worker.js` fetches
// and what `burrow_wasm_bg.wasm` exports.
//
// Split out of `globals-qpdf.d.ts` by #137, when a second bundle came to link qpdf: the engine
// is shared and stays there, while the module ids and the operations belong to this bundle
// alone. A declaration that outlives the module it describes type-checks and then fails at run
// time in the browser and nowhere else, so the redaction project must not see these.
//
// Included by `tsconfig.json` only. Everything here MERGES into the interfaces `globals.d.ts`
// declares.

/** The wasm modules the base bundle fetches. Generated into it as `BURROW_ENGINE_MODULE_IDS`. */
type EngineModuleId = "qpdfWasm" | "burrowWasm";

interface BurrowEngines {
  qpdfWasm: EngineEntry;
  burrowWasm: EngineEntry;
}

/** The operations the base artifact exports. */
interface BurrowWasm {
  /**
   * Open with **qpdf**, WITHOUT recovery, and report the page count. The render artifact has
   * its own, PDFium's (`globals-render.d.ts`); redaction's has none, which is why this is not
   * in the shared `globals.d.ts` any more (#137).
   */
  page_count(bytes: Uint8Array, password: Uint8Array | undefined, limits: WebLimits): Reply;
  /**
   * Every page's effective rotation, in page order.
   *
   * Exists for the differential conformance harness, and is honest surface rather than a test
   * hook: it opens a document and reports an attribute, exactly as `page_count` does. A
   * rotate case comparing only a page count would be vacuous, because a rotation cannot
   * change it.
   */
  page_rotations(bytes: Uint8Array, password: Uint8Array | undefined, limits: WebLimits): Reply;
  /**
   * Re-encode a document smaller, and say so when it could not be.
   *
   * A successful reply with NO output is the ordinary "already efficiently stored"
   * answer, not a failure. `originalBytes` and `producedBytes` carry the result either
   * way, so a page needs no second round trip.
   */
  compress(bytes: Uint8Array, password: Uint8Array | undefined, limits: WebLimits): Reply;
  structure_check(
    bytes: Uint8Array,
    password: Uint8Array | undefined,
    attemptRecovery: boolean,
    limits: WebLimits,
  ): Reply;
  /**
   * Merge several documents, given as one flat buffer plus a table of their lengths.
   *
   * Not an array of arrays: wasm-bindgen would copy every document twice, and a merge is
   * the largest thing this boundary carries. Rust validates the table against the buffer.
   */
  merge(inputs: Uint8Array, lengths: Uint32Array, limits: WebLimits): Reply;

  /**
   * Turn chosen pages of a document and return the result.
   *
   * `pages` is ONE-BASED, because that is how a person names a page. `degrees` is any
   * multiple of 90, negative or over 360; Rust reduces it and refuses anything else before
   * the document is opened, so a bad argument costs no parse.
   */
  rotate(
    bytes: Uint8Array,
    pages: Uint32Array,
    degrees: number,
    password: Uint8Array | undefined,
    limits: WebLimits,
  ): Reply;

  /**
   * Put a document's pages in a different order and return the result.
   *
   * `order` is ONE-BASED and names every page exactly once. Rust refuses anything else — the
   * wrong length, a page number of zero, one past the end, or one named twice — before the
   * document is opened, so a bad argument costs no parse.
   */
  reorder(
    bytes: Uint8Array,
    order: Uint32Array,
    password: Uint8Array | undefined,
    limits: WebLimits,
  ): Reply;
  /** ADR 0023: a split in progress, pulled one part at a time. */
  split_begin(
    bytes: Uint8Array,
    cuts: Uint32Array,
    password: Uint8Array | undefined,
    limits: WebLimits,
  ): SplitSession;
}
