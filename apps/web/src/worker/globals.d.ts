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
  /** The same memory as 32-bit floats, for reading an `FS_SIZEF` back out of the C stack. */
  HEAPF32: Float32Array;
  _malloc(size: number): number;
  _free(ptr: number): void;

  // NO `_FPDF_*` AND NO `_qpdf_*` HERE. Each engine's exports are declared by the `.d.ts`
  // that belongs to its bundle — `globals-qpdf.d.ts` and `globals-render.d.ts` — and TypeScript
  // merges whichever one this project includes into the interface above.
  //
  // That is not tidiness. The five `fpdfview.h` declarations stood in this file for exactly one
  // build after spike 0004 stopped loading the module, which is the state where a call to one
  // of them TYPE-CHECKS and then fails at run time in the browser and nowhere else. Splitting
  // them per bundle means the checker sees the same set of exports the bundle actually has.

  /**
   * Add a JS function to the module's function table and return its index.
   *
   * Needs a GROWABLE table, which Emscripten emits only with `ALLOW_TABLE_GROWTH`. The
   * vendored `pdfium.wasm` declares `min=3299` with no maximum, so it grows -- established by
   * reading the artifact's table section rather than assumed. Throws if it cannot grow.
   */
  addFunction(fn: (...args: number[]) => number, signature: string): number;
  /** Release a table entry from {@link addFunction}. Without it the table only ever grows. */
  removeFunction(index: number): void;

  // NO `stackSave`/`stackAlloc`/`stackRestore` HERE EITHER, and it is the same lesson as the
  // `_FPDF_*` note above, learned again. The qpdf build exports them and the vendored
  // `pdfium.wasm` does not -- 3 occurrences against 0, checked in the artifacts. While they sat
  // in this shared interface, a call to one of them from the PDFium bridge type-checked and
  // threw in the browser: every render came back `Internal`, the worker was discarded as
  // poisoned, and nothing but the differential corpus noticed. They are in
  // `globals-qpdf.d.ts`.
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
  // THE WASM ENTRIES ARE PER BUNDLE, declared in `globals-qpdf.d.ts` / `globals-render.d.ts`.
  // A bundle's generated manifest carries only its own, which is what makes "the base bundle
  // cannot fetch PDFium" a fact about the file rather than a promise about the code.
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

/**
 * The wasm modules THIS bundle fetches, in order, generated beside the manifest.
 *
 * `prelude.js` loops over it, and `ENGINE_MODULE_COUNTS` in the page-side manifest is its
 * length — so how many `{ starting: true }` messages a bundle sends and how many the host
 * will honour come from one array. `EngineModuleId` is declared per bundle, so a project that
 * includes the qpdf declarations cannot name a PDFium module and vice versa.
 */
declare const BURROW_ENGINE_MODULE_IDS: readonly EngineModuleId[];

// `prelude.js` DEFINES `INHERITS_PAGE_CSP`, `silent`, `compiled` and `instantiateFrom`, so
// they are not declared here -- redeclaring a `const` that a checked file also declares is an
// error, and the definitions carry their own JSDoc.

// --- the bridge ---
/**
 * Wire the bridge to the initialised module. Defined once per bundle, by its own bridge half.
 *
 * Declared here rather than twice, because a `declare function` cannot be repeated in one
 * program — and because the signature is genuinely the same either way: one engine module,
 * attached once, by the file that owns that engine's globals.
 */
declare function __burrow_attach(module: EmscriptenModule): void;

// --- the wasm-bindgen glue (`--target no-modules`) ---
interface BurrowWasm {
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
  /**
   * Whether a set of inputs is small enough in total, by size alone.
   *
   * Called before a single `Blob` is read, so the transport cannot exhaust the tab on the way
   * to a ceiling the core would have applied anyway (issue #51). The comparison itself is
   * `burrow_core::ops::check_total_input_bytes`, the same function `merge` calls.
   */
  check_input_budget(sizes: Float64Array, limits: WebLimits): Reply;
  /**
   * Open a document and report its page count.
   *
   * IN BOTH ARTIFACTS, answered by a different engine in each — qpdf in the base one, PDFium
   * in the render one. That is why it is declared here rather than twice: the signature is
   * the same, the engine is the artifact's business, and ADR 0009 §2 keeps engine choice out
   * of the protocol.
   */
  page_count(bytes: Uint8Array, password: Uint8Array | undefined, limits: WebLimits): Reply;
}

declare const wasm_bindgen: BurrowWasm;

/** Consumed by the call it is passed to — see the note in `main.js`. Never `.free()`d. */
interface WebLimits {
  free(): void;
}

/** Returned owned, so it MUST be freed. */

/**
 * A split in progress. `parts` is known before the first part is; `next_part` produces, verifies
 * and returns one; a reply with `ok === false` means the WHOLE split failed (ADR 0023 §3).
 */
interface SplitSession {
  readonly ok: boolean;
  readonly parts: number;
  begin_reply(): Reply;
  next_part(): Reply;
  free(): void;
}

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
  readonly engine_heap_bytes: bigint;
  /** Which input failed, or -1. Lets a page mark a file without parsing prose. */
  readonly failedInput: number;
  /** Every page's effective rotation, in page order. `page_rotations` only. */
  readonly rotations: BigInt64Array;
  /** What was wrong with that input, or empty. */
  readonly innerKind: string;
  /**
   * What the caller handed in, in bytes. `compress` only; zero elsewhere.
   */
  readonly originalBytes: bigint;
  /**
   * What the re-encoding came to, whether or not it was kept. `compress` only.
   *
   * Read this rather than `outputLength`: when the re-encoding was not kept there is no
   * output to measure, and this is the only record of what it weighed.
   */
  readonly producedBytes: bigint;
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
  // NO ENGINE BRIDGE GLOBALS HERE. `__burrow_qpdf_*` and `__burrow_pdfium_*` are declared by
  // their own bundle's `.d.ts`, for the reason `EmscriptenModule` gives above: a declaration
  // that outlives the module it describes type-checks and then fails in the browser.

  // --- the clock ---
  __burrow_now_ms(): bigint;
}
