// A stubbed worker scope, so `main.js` can be driven without a browser.
//
// Test-only, and it lives under `src/` because that is the only place vitest's `include` glob
// looks. Extracted from `init-memoisation.test.ts` when `input-budget.test.ts` needed the same
// scope: two copies of a rig this fiddly would be two things to keep in step, and the second
// would be the one that drifted.
//
// THE STUBS ARE DELIBERATELY DUMB. Nothing here decides anything, so a case that passes is
// reporting on `main.js` rather than on this file. The two pieces of behaviour are the gate
// that holds `createQpdfModule()` open -- which is what lets a second message arrive while
// init is still in flight -- and the read counter, which is how a test can ask whether a
// file's bytes were touched.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";

const here = dirname(fileURLToPath(import.meta.url));

/**
 * The base bundle's worker code, in the order `stage-web-engines.mjs` concatenates it.
 *
 * TWO FILES, NOT ONE, SINCE ADR 0026. `main.js` used to hold the whole protocol; it now holds
 * this bundle's `init`, `KNOWN_OPS` and `runOperation`, while `worker-protocol.js` holds the
 * guard, the memoised promise, the ack and the reply flattening — and is byte-identical in the
 * render bundle.
 *
 * Joined here in the SAME ORDER the bundle uses, because that is what makes a case that passes
 * a statement about what ships. A scope built from `main.js` alone would have no `onmessage` at
 * all, which is how this was noticed rather than shipped.
 */
export const MAIN = [
  readFileSync(join(here, "worker-protocol.js"), "utf8"),
  readFileSync(join(here, "main.js"), "utf8"),
].join("\n");

/** The render bundle's worker code, same order, same reason. */
export const RENDER_MAIN = [
  readFileSync(join(here, "worker-protocol.js"), "utf8"),
  readFileSync(join(here, "render-main.js"), "utf8"),
].join("\n");

/**
 * A `Reply` as `drainReply` reads it. Field names match the wasm-bindgen getters.
 *
 * Exported so a case can plant a refusal without restating twelve fields, and so a field
 * added to `Reply` is added here once.
 */
export function replyShape(): Record<string, unknown> {
  return {
    ok: true,
    kind: "Ok",
    fatal: false,
    message: "",
    pages: 0,
    limit: "",
    stage: "",
    requested: 0n,
    allowed: 0n,
    recycle: false,
    failed_input: -1,
    inner_kind: "",
    output_length: 0,
    engine_heap_bytes: 0n,
    // EVERY FIELD `drainReply` READS, or the stub is stale in exactly the way a rebuilt-Rust
    // module would be -- `Array.from(undefined)` throws, the reply arrives as `Internal`, and
    // the test fails somewhere unrelated to what it was testing. That is what happened when
    // `rotations` was added: two suites went red on "expected LimitExceeded, got Internal".
    // The guard around `drainReply` is what turned it into a failure rather than a crash.
    rotations: new BigInt64Array(0),
    // THE SAME LESSON, ONE OPERATION LATER. `compress` added two counts that `drainReply`
    // calls `.toString()` on, so omitting them here throws exactly as `Array.from(undefined)`
    // did -- and the suites go red on "expected LimitExceeded, got Internal", nowhere near
    // what they were testing. Named as `drainReply` reads them, which is the wasm-bindgen
    // getter spelling rather than the Rust field's.
    originalBytes: 0n,
    producedBytes: 0n,
    take_output: () => null,
    free: () => {},
  };
}

/** A promise plus its resolver, so a test can hold init open. */
export function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

export interface Attachment {
  qpdf: unknown;
}

export interface Harness {
  /**
   * The evaluated worker scope.
   *
   * Exposed so a test can ask WHICH names a bundle supplied, which is the contract between
   * `worker-protocol.js` and the file concatenated after it — three hoisted names that nothing
   * else can assert, because the protocol is the half that does the reading.
   */
  scope: Record<string, unknown>;
  /** How many times a request's bytes were actually read. The guard's real question. */
  blobReads: () => number;
  /** The sizes handed to `check_input_budget`, per call. Length IS the call count. */
  budgetCalls: number[][];
  /** The options object each `createQpdfModule()` call was given. */
  qpdfOptions: { printErr?: unknown; print?: unknown }[];
  /** Every object `createQpdfModule()` has handed back. Length IS the instance count. */
  qpdfInstances: object[];
  /** Every `__burrow_attach` call, in order. */
  attaches: Attachment[];
  /** Everything the worker posted to the page. */
  posted: Record<string, unknown>[];
  /** Let the pending `createQpdfModule()` settle. */
  releaseQpdf: () => void;
  /** `ensureReady`, reachable because a function declaration is a property of the scope. */
  ensureReady: () => Promise<unknown>;
  /** Deliver a message the way the browser would. */
  send: (request: Record<string, unknown>) => Promise<void>;
  /**
   * The lexically-scoped names the bundle declared, by evaluating an expression beside it.
   *
   * `const` and `let` in a `runInNewContext` script live in that script's own scope and never
   * become properties of the context object -- which is exactly how the real bundle works, one
   * concatenated script sharing a lexical scope. So asking whether `KNOWN_OPS` exists means
   * asking from inside, and `guard.test.ts` reads `CREATED_FROM_BLOB` the same way.
   */
  declared: () => Record<string, unknown>;
  /** A `page_count` request wired to this harness's read counter. */
  op: (id: number) => Record<string, unknown>;
}

/**
 * Evaluate a copy of `main.js` against a stubbed worker scope.
 *
 * The stubs are deliberately dumb: nothing here decides anything, so a case that passes is
 * reporting on `main.js` rather than on the harness. The one piece of behaviour is that
 * `createQpdfModule` does not settle until `releaseQpdf()` is called -- which is what lets a
 * second message arrive while init is still in flight, the exact window the memoised promise
 * exists to close.
 */
export function load(
  source: string,
  policed = true,
  /**
   * What `check_input_budget` answers. Defaults to "within budget".
   *
   * A function rather than a value so a case can count the calls, and so a refusal can be
   * planted without building a second scope.
   */
  budget: () => Record<string, unknown> = () => ({ ...replyShape(), ok: true }),
): Harness {
  const qpdfInstances: object[] = [];
  const attaches: Attachment[] = [];
  const posted: Record<string, unknown>[] = [];
  const gate = deferred<void>();
  let blobReads = 0;
  const qpdfOptions: { printErr?: unknown; print?: unknown }[] = [];
  const budgetCalls: number[][] = [];

  // A Reply as `drainReply` reads it. Field names match the wasm-bindgen getters.
  const reply = () => ({ ...replyShape(), pages: 1 });

  const wasm_bindgen = Object.assign(async () => {}, {
    min_converging_memory_bytes: () => 0n,
    default_limits: () => ({
      max_input_bytes: 0n,
      max_memory_bytes: 0n,
      max_duration_ms: 0n,
      max_pages: 0n,
      max_pixels: 0n,
      free: () => {},
    }),
    WebLimits: class {},
    page_count: reply,
    structure_check: reply,
    merge: reply,
    // THE PRE-FLIGHT. It is stubbed here because `main.js` calls it on every operation, and a
    // scope that lacked it would fail every case in every file for a reason unrelated to what
    // the case is about.
    check_input_budget: (sizes: Float64Array) => {
      budgetCalls.push([...sizes]);
      return budget();
    },
  });

  const scope: Record<string, unknown> = {
    // `self === globalThis` in a real worker, as `guard.test.ts` also models. Keeping them
    // distinct would let a refactor of `self.postMessage(...)` to a bare `postMessage(...)`
    // -- valid in a worker -- fail here for a reason that does not exist in production.
    onmessage: null as unknown,
    postMessage: (message: Record<string, unknown>) => posted.push(message),
    POLICED: Promise.resolve(
      policed ? { policed: true, reason: "" } : { policed: false, reason: "probe-succeeded" },
    ),
    silent: { printErr: () => {}, print: () => {} },
    instantiateFrom: () => () => {},
    // BOTH BUNDLES' MODULE IDS. The base one awaits `compiled["burrowWasm"]` and the render
    // one `compiled["burrowRenderWasm"]`; a scope with only the first makes a render case fail
    // on an `await undefined` far from what it was testing.
    compiled: { burrowWasm: Promise.resolve({}), burrowRenderWasm: Promise.resolve({}) },
    createQpdfModule: async (options: { printErr?: unknown; print?: unknown }) => {
      // ADR 0006 requirement 2: the glue is handed `...silent`, which is what keeps qpdf's
      // object numbers and byte offsets out of the devtools console. Asserted here because a
      // stub that ignores its argument would stay green if `...silent` were dropped.
      qpdfOptions.push(options);
      await gate.promise;
      const instance = { id: qpdfInstances.length };
      qpdfInstances.push(instance);
      return instance;
    },
    __burrow_attach: (qpdf: unknown) => attaches.push({ qpdf }),
    // WHAT `render-prelude.js` WOULD HAVE PUT THERE. The render bundle's `init()` awaits
    // `pdfiumReady` and calls `_FPDF_InitLibrary` on what it resolves to; the prelude is not in
    // this concatenation for the same reason `prelude.js` is not — the stubs above model it.
    //
    // `calledRun: true` models the warm-cache ordering, which is the one a real origin
    // produces. `render-init.test.ts` is where the ORDERING itself is the subject; here it is
    // scenery, and picking the ordering that happens in production is the right scenery.
    Module: { calledRun: true, _FPDF_InitLibrary: () => {} },
    pdfiumReady: Promise.resolve({ _FPDF_InitLibrary: () => {} }),
    resolvePdfium: () => {},
    wasm_bindgen,
  };
  scope["self"] = scope;
  scope["globalThis"] = scope;

  runInNewContext(source, scope);

  // Evaluated separately so a bundle that fails to declare one of them throws HERE, naming it,
  // rather than taking the whole `load()` down.
  const declared = () =>
    runInNewContext(
      `${source}\n;({ init: typeof init === "function" ? init : undefined,` +
        ` KNOWN_OPS: typeof KNOWN_OPS !== "undefined" ? KNOWN_OPS : undefined,` +
        ` runOperation: typeof runOperation === "function" ? runOperation : undefined,` +
        ` ensureReady: typeof ensureReady === "function" ? ensureReady : undefined })`,
      { ...scope, self: scope, globalThis: scope },
    ) as Record<string, unknown>;

  const worker = scope as unknown as {
    onmessage: (event: { data: unknown }) => Promise<void>;
  };

  return {
    scope,
    declared,
    blobReads: () => blobReads,
    budgetCalls,
    qpdfOptions,
    qpdfInstances,
    attaches,
    posted,
    releaseQpdf: gate.resolve,
    ensureReady: scope["ensureReady"] as () => Promise<unknown>,
    send: (request) => worker.onmessage({ data: request }),
    op: (id: number) => operation(id, () => (blobReads += 1)),
  };
}

/** A `page_count` request with everything the handler reads. */
export function operation(id: number, onRead: () => void = () => {}) {
  return {
    id,
    op: "page_count",
    blob: {
      // `size` is what the budget check reads, and it is deliberately the same number the
      // read below returns: a stub whose size disagreed with its bytes could make an
      // ordering test pass for the wrong reason.
      size: 8,
      arrayBuffer: async () => {
        onRead();
        return new ArrayBuffer(8);
      },
    },
    limits: {
      maxInputBytes: 1,
      maxMemoryBytes: 1,
      maxDurationMs: 1,
      maxPages: 1,
      maxPixels: 1,
    },
  };
}
