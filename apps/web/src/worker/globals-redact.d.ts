// The redaction bundle's half of the worker's ambient declarations: what
// `burrow-redact-worker.js` fetches and what `burrow_wasm_redact_bg.wasm` exports (#137).
//
// Included by `tsconfig.redact.json` only, beside `globals-qpdf.d.ts` -- this bundle links qpdf
// through the same bridge as the base one -- and never beside `globals-documents.d.ts`: the
// redaction module exports none of the base operations, and a declaration that outlives the
// module it describes type-checks and then fails at run time in the browser and nowhere else.
//
// Everything here MERGES into the interfaces `globals.d.ts` declares.

/** The wasm modules the redaction bundle fetches. Generated into it as `BURROW_ENGINE_MODULE_IDS`. */
type EngineModuleId = "qpdfWasm" | "burrowRedactWasm";

interface BurrowEngines {
  qpdfWasm: EngineEntry;
  burrowRedactWasm: EngineEntry;
}

/**
 * A region on the displayed page, in display units from its top-left.
 *
 * Constructed here and CONSUMED by `redact`: wasm-bindgen moves a struct argument into Rust, so
 * calling `free()` afterwards is a double free -- the same rule `WebLimits` carries in `main.js`.
 */
interface WebRegion {
  free(): void;
}

/** The one operation the redaction artifact exports. */
interface BurrowWasm {
  /**
   * Clear `region` on `page`, and return the document only if it reads back as promised.
   *
   * `page` and `covered` are 1-based page numbers; `page` must be in `covered`, which decides
   * which fonts may be cut. On success the reply carries the verified document and the report.
   */
  redact(
    bytes: Uint8Array,
    page: number,
    covered: Uint32Array,
    region: WebRegion,
    password: Uint8Array | undefined,
    limits: WebLimits,
  ): Reply;
  WebRegion: new (left: number, top: number, width: number, height: number) => WebRegion;
}
