// The shapes the worker bundle relies on, declared once.
//
// These files are plain JavaScript concatenated into one worker script at build time, so
// nothing here is imported — every declaration below describes a global that exists because
// an earlier file in the bundle created it. That is unusual, and it is why the bridge went
// unchecked until M1 PR 4a-i: it is the file that computes heap offsets and pointer values,
// and it had no static checking of any kind.
//
// `tsconfig.json` in this directory checks these with `lib: ["WebWorker"]`. The app's own
// tsconfig cannot: it uses the DOM lib, under which `self` is a `Window`.

/**
 * What is handed TO an Emscripten module before it starts.
 *
 * Separate from {@link EmscriptenModule} because the glue genuinely works this way: you
 * assign a plain configuration object to the global, the glue reads it at load time, and
 * then *populates the same object* with the exports. Before `onRuntimeInitialized` there is
 * no `_malloc` on it; afterwards there is.
 *
 * Modelling that as one type would mean either pretending the config has exports (and
 * casting at every assignment) or making the exports optional (and null-checking at every
 * call). Two types with one documented narrowing, where the guarantee actually holds, is
 * the honest shape.
 */
interface EmscriptenConfig {
  /** Set by the glue once the runtime is up. */
  calledRun?: boolean;
  onRuntimeInitialized?: () => void;

  /** Silencing hooks. ROADMAP item 7's web half. */
  print?: (text: string) => void;
  printErr?: (text: string) => void;

  /**
   * Supply an already-compiled module instead of letting the glue fetch or locate one.
   *
   * Returning `{}` tells Emscripten the instantiation is asynchronous and that `done` will
   * be called later.
   */
  instantiateWasm?: (
    imports: WebAssembly.Imports,
    done: (instance: WebAssembly.Instance, module: unknown) => void,
  ) => object;
}

/** The subset of a *running* Emscripten module burrow uses. */
interface EmscriptenModule extends EmscriptenConfig {
  /** Bytes of the module's linear memory. Re-read after every allocation: growth detaches it. */
  HEAPU8: Uint8Array;
  /** The same memory as 32-bit words, for writing an out-parameter. */
  HEAPU32: Uint32Array;
  _malloc(size: number): number;
  _free(ptr: number): void;

  // --- PDFium. `fpdfview.h`. ---
  _FPDF_InitLibrary(): void;
  _FPDF_LoadMemDocument64(data: number, size: number, password: number): number;
  _FPDF_GetLastError(): number;
  _FPDF_GetPageCount(doc: number): number;
  _FPDF_CloseDocument(doc: number): void;

  // --- qpdf. Exactly the C API `core/burrow-engines/src/qpdf/ffi.rs` declares, which is
  //     the set ADR 0013 verified routes through qpdf's `trap_errors`. ---
  _qpdf_init(): number;
  _qpdf_cleanup(dataPtr: number): void;
  _qpdf_silence_errors(data: number): void;
  _qpdf_set_suppress_warnings(data: number, value: number): void;
  _qpdf_set_logger(data: number, logger: number): void;
  _qpdf_set_attempt_recovery(data: number, value: number): void;
  _qpdf_read_memory(
    data: number,
    description: number,
    buffer: number,
    size: bigint,
    password: number,
  ): number;
  _qpdf_has_error(data: number): number;
  _qpdf_get_error(data: number): number;
  _qpdf_get_error_code(data: number, error: number): number;
  _qpdf_get_num_pages(data: number): number;
  _qpdf_global_set_uint32(param: number, value: number): number;
  _qpdflogger_create(): number;
  _qpdflogger_set_info(logger: number, dest: number, a: number, b: number): void;
  _qpdflogger_set_warn(logger: number, dest: number, a: number, b: number): void;
  _qpdflogger_set_error(logger: number, dest: number, a: number, b: number): void;

  // --- the C stack, for an out-parameter that lives for one call. ---
  stackSave(): number;
  stackAlloc(size: number): number;
  stackRestore(saved: number): void;
}

/** One staged artifact: where it is, and what it must hash to. */
interface EngineEntry {
  url: string;
  integrity: string;
  bytes: number;
}

/**
 * The manifest generated into the bundle by `tools/stage-web-engines.mjs`.
 *
 * Named fields rather than a `Record`, so `probeOrigin` is a string and the three artifacts
 * are entries — a `Record<string, EngineEntry>` typed the origin as an entry and the
 * distinction only showed up when something tried to use it.
 */
interface BurrowEngines {
  pdfiumWasm: EngineEntry;
  qpdfWasm: EngineEntry;
  burrowWasm: EngineEntry;
  /**
   * A few bytes, allowlisted, existing only as the guard's control.
   *
   * Dedicated rather than reusing an engine fetch: an engine response can be served from the
   * HTTP cache, and a cached control would succeed while the probe failed for network
   * reasons — reporting "policed" with nothing enforcing anything.
   */
  control: EngineEntry;
  /** The origin this build's CSP was generated against; the guard probes it. */
  probeOrigin: string;
}

declare const BURROW_ENGINES: BurrowEngines;

// `prelude.js` DEFINES `INHERITS_PAGE_CSP`, `silent`, `compiled` and `instantiateFrom`, so
// they are not declared here -- redeclaring a `const` that a checked file also declares is an
// error, and the definitions carry their own JSDoc.

// --- bridge.js ---
declare function __burrow_attach(pdfium: EmscriptenModule, qpdf: EmscriptenModule): void;

// --- the Emscripten glue, concatenated into the bundle ---
/** The global PDFium reads at load time, and then fills in. */
declare const Module: EmscriptenConfig;
declare function createQpdfModule(options: object): Promise<EmscriptenModule>;

// --- the wasm-bindgen glue (`--target no-modules`) ---
declare const wasm_bindgen: {
  (init: { module_or_path: WebAssembly.Module }): Promise<unknown>;
  WebLimits: new (
    maxInputBytes: bigint,
    maxMemoryBytes: bigint,
    maxDurationMs: bigint,
    maxPages: bigint,
    maxPixels: bigint,
  ) => WebLimits;
  page_count(bytes: Uint8Array, password: Uint8Array | undefined, limits: WebLimits): Reply;
  structure_check(
    bytes: Uint8Array,
    password: Uint8Array | undefined,
    attemptRecovery: boolean,
    limits: WebLimits,
  ): Reply;
};

/** Consumed by the call it is passed to — see the note in `main.js`. Never `.free()`d. */
interface WebLimits {
  free(): void;
}

/** Returned owned, so it MUST be freed. */
interface Reply {
  readonly ok: boolean;
  readonly kind: string;
  readonly fatal: boolean;
  readonly message: string;
  readonly pages: bigint;
  readonly limit: string;
  readonly requested: bigint;
  readonly allowed: bigint;
  /** ADR 0009's lifecycle verdict, computed in Rust. See `recycle.rs`. */
  readonly recycle: boolean;
  readonly pdfium_heap_bytes: bigint;
  readonly qpdf_heap_bytes: bigint;
  free(): void;
}

/**
 * The bridge's JavaScript surface, declared once.
 *
 * **This mirrors `PdfiumBridge` and `QpdfBridge` in
 * `core/burrow-engines/src/web/bridge.rs`**, which is the audit surface ADR 0009 relies on:
 * every capability the binding has is one entry there and one entry here. A name in one and
 * not the other is a mismatch worth noticing.
 *
 * Declaring the signatures here also contextually types the arrow functions in `bridge.js`,
 * so the file that computes heap offsets is checked rather than inferred as `any`.
 */
interface WorkerGlobalScope {
  /** Assigned as a config; populated by the glue. See {@link EmscriptenConfig}. */
  Module: EmscriptenConfig;

  // --- PDFium ---
  __burrow_pdfium_copy_in(bytes: Uint8Array): number;
  __burrow_pdfium_wipe_free(ptr: number, len: number): void;
  __burrow_pdfium_free_input(ptr: number): void;
  /** `(code << 32) | handle` — both values from one call. See `bridge.js`. */
  __burrow_pdfium_load(data: number, len: number, password: number): bigint;
  __burrow_pdfium_pages(doc: number): number;
  __burrow_pdfium_close(doc: number, data: number): void;
  __burrow_pdfium_heap_pages(): number;

  // --- qpdf ---
  __burrow_qpdf_copy_in(bytes: Uint8Array): number;
  __burrow_qpdf_free(ptr: number): void;
  __burrow_qpdf_wipe_free(ptr: number, len: number): void;
  __burrow_qpdf_init(): number;
  __burrow_qpdf_cleanup(data: number): void;
  __burrow_qpdf_silence_errors(data: number): void;
  __burrow_qpdf_set_suppress_warnings(data: number, value: number): void;
  __burrow_qpdf_set_logger(data: number, logger: number): void;
  __burrow_qpdf_set_attempt_recovery(data: number, value: number): void;
  __burrow_qpdf_read_memory(
    data: number,
    description: number,
    buffer: number,
    size: bigint,
    password: number,
  ): number;
  __burrow_qpdf_has_error(data: number): number;
  __burrow_qpdf_get_error(data: number): number;
  __burrow_qpdf_get_error_code(data: number, error: number): number;
  __burrow_qpdf_get_num_pages(data: number): number;
  __burrow_qpdf_global_set_uint32(param: number, value: number): number;
  __burrow_qpdflogger_create(): number;
  __burrow_qpdflogger_discard_all(logger: number, destination: number): void;
  __burrow_qpdf_heap_pages(): number;

  // --- the clock ---
  __burrow_now_ms(): bigint;
}
