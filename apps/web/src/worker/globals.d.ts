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
  // The write path, M1 PR B2. Every one matches an entry in `build-wasm.sh`'s
  // EXPORTED_FUNCTIONS allowlist -- a symbol not on that list is not in the module at all.
  _qpdf_get_page_n(data: number, n: number): number;
  _qpdf_add_page(data: number, source: number, page: number, first: number): number;
  _qpdf_init_write_memory(data: number): number;
  /** Note the upstream spelling: `ID` is capitalised in qpdf's C API. */
  _qpdf_set_deterministic_ID(data: number, value: number): void;
  _qpdf_write(data: number): number;
  _qpdf_get_buffer_length(data: number): number;
  _qpdf_get_buffer(data: number): number;
  // The object-handle API, added for `rotate`. `key` is a POINTER to a NUL-terminated
  // string in the module's heap, not a JS string: Rust copies the key in and passes the
  // address, so nothing on this side builds or chooses a key.
  _qpdf_oh_get_key(data: number, oh: number, key: number): number;
  _qpdf_oh_get_type_code(data: number, oh: number): number;
  /** `long long` on the C side; the module is built -sWASM_BIGINT=1, so this is a BigInt. */
  _qpdf_oh_get_int_value(data: number, oh: number): bigint;
  _qpdf_oh_new_integer(data: number, value: bigint): number;
  _qpdf_oh_replace_key(data: number, oh: number, key: number, item: number): void;
  _qpdf_oh_release(data: number, oh: number): void;
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
  min_converging_memory_bytes(): bigint;
  default_limits(): WebLimits & {
    readonly max_input_bytes: bigint;
    readonly max_memory_bytes: bigint;
    readonly max_duration_ms: bigint;
    readonly max_pages: bigint;
    readonly max_pixels: bigint;
  };
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
   * Whether a set of inputs is small enough in total, by size alone.
   *
   * Called before a single `Blob` is read, so the transport cannot exhaust the tab on the way
   * to a ceiling the core would have applied anyway (issue #51). The comparison itself is
   * `burrow_core::ops::check_total_input_bytes`, the same function `merge` calls.
   */
  check_input_budget(sizes: Float64Array, limits: WebLimits): Reply;
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
  /** Which check fired. See `burrow_types::Stage`. Empty unless `kind` is LimitExceeded. */
  readonly stage: string;
  readonly requested: bigint;
  readonly allowed: bigint;
  /** ADR 0009's lifecycle verdict, computed in Rust. See `recycle.rs`. */
  readonly recycle: boolean;
  readonly pdfium_heap_bytes: bigint;
  readonly qpdf_heap_bytes: bigint;
  /** Which input failed, or -1. Lets a page mark a file without parsing prose. */
  readonly failedInput: number;
  /** Every page's effective rotation, in page order. `page_rotations` only. */
  readonly rotations: BigInt64Array;
  /** What was wrong with that input, or empty. */
  readonly innerKind: string;
  /** Bytes in the produced document, without taking it. Zero if there is none. */
  readonly outputLength: number;
  /**
   * Take the produced document, leaving the reply empty.
   *
   * MOVES rather than copies, which is why it is a method and not a getter: a merged PDF is
   * the largest thing this boundary carries, and reading it twice would double the peak. A
   * second call returns an empty array, which is why `outputLength` is read first.
   */
  takeOutput(): Uint8Array<ArrayBuffer>;
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
  /** Wipes before freeing: this buffer holds the user's document. */
  __burrow_pdfium_free_input(ptr: number, len: number): void;
  /** `(code << 32) | handle` — both values from one call. See `bridge.js`. */
  __burrow_pdfium_load(data: number, len: number, password: number): bigint;
  __burrow_pdfium_pages(doc: number): number;
  /** Closes, then wipes and frees `data` — the user's document. See `bridge.js`. */
  __burrow_pdfium_close(doc: number, data: number, len: number): void;
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
  __burrow_qpdf_get_page_n(data: number, n: number): number;
  __burrow_qpdf_add_page(data: number, source: number, page: number, first: number): number;
  __burrow_qpdf_init_write_memory(data: number): number;
  __burrow_qpdf_set_deterministic_id(data: number, value: number): void;
  __burrow_qpdf_write(data: number): number;
  __burrow_qpdf_get_buffer_length(data: number): number;
  __burrow_qpdf_get_buffer(data: number): number;
  __burrow_qpdf_oh_get_key(data: number, oh: number, key: number): number;
  __burrow_qpdf_oh_get_type_code(data: number, oh: number): number;
  __burrow_qpdf_oh_get_int_value(data: number, oh: number): bigint;
  __burrow_qpdf_oh_new_integer(data: number, value: bigint): number;
  __burrow_qpdf_oh_replace_key(data: number, oh: number, key: number, item: number): void;
  __burrow_qpdf_oh_release(data: number, oh: number): void;
  /** The only bridge function that carries bytes OUT of an engine heap. */
  __burrow_qpdf_copy_out(ptr: number, len: number): Uint8Array;
  __burrow_qpdf_global_set_uint32(param: number, value: number): number;
  __burrow_qpdflogger_create(): number;
  __burrow_qpdflogger_discard_all(logger: number, destination: number): void;
  __burrow_qpdf_heap_pages(): number;

  // --- the clock ---
  __burrow_now_ms(): bigint;
}
