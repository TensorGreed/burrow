// Init runs ONCE per worker, and this is the only thing that says so.
//
// Spike 0001's HIGH finding: `pdfium.js` loaded twice in one scope rebinds every glue global
// while the bridge still holds the first instance, so calls read and write the *other*
// instance's linear memory. The sandbox holds; parses come back confidently wrong.
//
// Spike 0003 (docs/spikes/0003-pdfium-source-build.md, Finding 3) asked whether one instance
// per worker closes that, and found the two engines in different positions:
//
//   * PDFium is closed STRUCTURALLY. Its glue is not modularised, it begins instantiating as
//     the bundle is parsed, and a worker scope evaluates the bundle exactly once. There is no
//     second `importScripts` to make, because there is no `importScripts`.
//     Two tests fail if that is undone: production-build's "ships the worker as ONE bundle,
//     with no glue loose beside it", and integrity's "the worker bundle contains all worker
//     code, so one digest covers it". Named rather than cited by line, which rots.
//   * qpdf is NOT. `qpdf.js` is MODULARIZE'd, so `createQpdfModule()` is an ordinary function
//     returning a fresh, fully independent instance on every call. Spike 0001 called qpdf
//     "immune" for that reason, and it is -- to the GLUE-GLOBAL mechanism it was describing.
//     The bridge is a different route to the same outcome: `__burrow_attach` assigns one
//     `qpdfModule`, so a second instance rebinds what every later `qpdf()` call returns while
//     an in-flight operation still holds pointers into the first instance's heap. The only thing preventing a
//     second call is `ready ??= init()` in `main.js` -- and when spike 0003 looked, **no test
//     would have failed if that reverted to a boolean flag set after the `await`**.
//
// So this file exists for qpdf, and the last case in it is the point: it applies exactly that
// revert to a copy of `main.js` and asserts the cases above would have caught it. A test for a
// one-line invariant is worth little unless it has been shown to fail when the line changes.
//
// Driven through `self.onmessage` rather than by calling `ensureReady()` alone, because "one
// instance across operations" is a claim about the operation path.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";

import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const MAIN = readFileSync(join(here, "main.js"), "utf8");

/** A promise plus its resolver, so a test can hold init open. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

interface Attachment {
  pdfium: unknown;
  qpdf: unknown;
}

interface Harness {
  /** How many times a request's bytes were actually read. The guard's real question. */
  blobReads: () => number;
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
function load(source: string, policed = true): Harness {
  const qpdfInstances: object[] = [];
  const attaches: Attachment[] = [];
  const posted: Record<string, unknown>[] = [];
  const gate = deferred<void>();
  let blobReads = 0;
  const qpdfOptions: { printErr?: unknown; print?: unknown }[] = [];

  const pdfiumModule = {
    calledRun: true,
    _FPDF_InitLibrary: () => {},
  };

  // A Reply as `drainReply` reads it. Field names match the wasm-bindgen getters.
  const reply = () => ({
    ok: true,
    kind: "Ok",
    fatal: false,
    message: "",
    pages: 1,
    limit: "",
    stage: "",
    requested: 0n,
    allowed: 0n,
    recycle: false,
    pdfium_heap_bytes: 0n,
    qpdf_heap_bytes: 0n,
    free: () => {},
  });

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
    Module: pdfiumModule,
    silent: { printErr: () => {}, print: () => {} },
    instantiateFrom: () => () => {},
    compiled: { burrowWasm: Promise.resolve({}) },
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
    __burrow_attach: (pdfium: unknown, qpdf: unknown) => attaches.push({ pdfium, qpdf }),
    wasm_bindgen,
  };
  scope["self"] = scope;
  scope["globalThis"] = scope;

  runInNewContext(source, scope);

  const worker = scope as unknown as {
    onmessage: (event: { data: unknown }) => Promise<void>;
  };

  return {
    blobReads: () => blobReads,
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
function operation(id: number, onRead: () => void = () => {}) {
  return {
    id,
    op: "page_count",
    blob: {
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

/**
 * `main.js` with the memoised promise reverted to a boolean flag set after the `await`.
 *
 * This is spike 0001's finding written out as code, and it is what the cases in this file
 * have to be able to see. Per CLAUDE.md, the mutation is asserted to have applied before it
 * is used: a `replace` that matched nothing is indistinguishable from a defence that holds,
 * and has twice produced a green suite that was testing nothing.
 */
function withBooleanFlag(): string {
  const memoised = "  ready ??= init();\n  return ready;";
  const flag = "  if (ready) return;\n  await init();\n  ready = true;";
  const signature = "function ensureReady() {";

  expect(MAIN, "main.js no longer contains the memoised assignment").toContain(memoised);
  expect(MAIN, "ensureReady's signature has changed").toContain(signature);

  const mutated = MAIN.replace(memoised, flag).replace(signature, `async ${signature}`);
  expect(mutated, "the mutation did not apply").not.toBe(MAIN);
  expect(mutated).toContain("ready = true;");
  return mutated;
}

describe("worker init is memoised, so the engines are instantiated once", () => {
  it("ensureReady returns the IDENTICAL promise across concurrent calls", async () => {
    const worker = load(MAIN);

    const first = worker.ensureReady();
    const second = worker.ensureReady();

    // Identity, not equivalence. Two distinct promises that both settle is exactly what a
    // boolean flag produces, and it is the bug.
    expect(second).toBe(first);

    worker.releaseQpdf();
    await first;
    expect(worker.qpdfInstances).toHaveLength(1);
  });

  it("two operations arriving before init finishes create ONE qpdf instance", async () => {
    const worker = load(MAIN);

    const both = Promise.all([worker.send(worker.op(1)), worker.send(worker.op(2))]);

    // ASSERT THE WINDOW IS OPEN BEFORE RELYING ON IT. Without this, `releaseQpdf()` runs
    // before either handler has reached `createQpdfModule` at all -- both are still suspended
    // on `await POLICED` -- so the concurrency this case is named for came from microtask
    // ordering rather than from the gate, and the comment saying otherwise was false.
    // Measured by a reviewer against an instrumented copy; kept as an assertion so it cannot
    // quietly stop being true.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(worker.qpdfInstances, "init should still be in flight here").toHaveLength(0);

    worker.releaseQpdf();
    await both;

    expect(worker.qpdfInstances).toHaveLength(1);
    expect(worker.attaches).toHaveLength(1);

    // AND BOTH OPERATIONS REACHED THE ENGINE. Without these the case passes with the whole
    // operation path broken: `main.js` catches a throwing operation into a content-free
    // `Internal`, which is indistinguishable from success to a case that only counts
    // instances. Measured -- a reviewer replaced the `page_count` stub with one that throws
    // and this case stayed green.
    expect(worker.posted.filter((m) => m["ack"] === true)).toHaveLength(2);
    expect(worker.posted.filter((m) => m["ok"] === true)).toHaveLength(2);
  });

  // MEASURED: THIS CASE ALONE DOES NOT CATCH THE REVERT, and that is worth saying rather
  // than leaving for someone to discover by deleting the two cases above. It releases the
  // gate before sending, so the operations are sequential -- and a boolean flag is perfectly
  // correct for sequential callers. Reverting `main.js` to a flag leaves this one GREEN while
  // the two cases above go red. It covers identity across operations; concurrency is theirs.
  it("the qpdf instance the bridge holds is identical across operations", async () => {
    const worker = load(MAIN);

    worker.releaseQpdf();
    await worker.send(worker.op(1));
    const afterFirst = worker.attaches.at(-1);
    await worker.send(worker.op(2));
    const afterSecond = worker.attaches.at(-1);

    expect(worker.attaches).toHaveLength(1);
    // The object identity is the property that matters: a second instance would have its own
    // linear memory, and the bridge would write to one heap while reading the other.
    expect(afterSecond?.qpdf).toBe(afterFirst?.qpdf);
    expect(afterSecond?.qpdf).toBe(worker.qpdfInstances[0]);

    // Both operations actually completed, so the identity above is not the identity of a
    // path nothing took. `ack` plus a reply for each of the two ids.
    expect(worker.posted.filter((m) => m["ack"] === true)).toHaveLength(2);
    expect(worker.posted.filter((m) => m["ok"] === true)).toHaveLength(2);
  });

  // NOT ABOUT MEMOISATION, AND HERE BECAUSE OF WHERE IT HAD TO GO.
  //
  // A security review measured this: with the fail-closed guard DELETED from `main.js`
  // entirely, every case above still passes -- 1 instance, 1 attach, 2 acks, 2 oks are
  // exactly the numbers they assert. The worker would read file bytes under no policy at all
  // and this suite would stay green.
  //
  // It cannot be asserted anywhere else. `guard.test.ts` evaluates `prelude.js` alone, so it
  // covers how the verdict is COMPUTED but never how `main.js` CONSUMES it; and the e2e side
  // cannot reach it, because `worker-src` no longer carries `'self'` so the browser refuses
  // to construct a URL worker before `main.js` runs. This file is the first harness in the
  // repository that evaluates `main.js`, which makes it the only place the consumer of the
  // verdict can be tested.
  //
  // The assertion that matters is `blobReads`, not the reply: the question is whether the
  // worker TOUCHED THE FILE, not whether it said no politely.
  it("refuses to touch the file when no policy is in force", async () => {
    const worker = load(MAIN, false);

    worker.releaseQpdf();
    await worker.send(worker.op(1));

    expect(worker.blobReads(), "the worker read file bytes under no policy").toBe(0);
    expect(worker.qpdfInstances, "the engines were initialised anyway").toHaveLength(0);
    expect(worker.posted).toHaveLength(1);
    expect(worker.posted[0]).toMatchObject({ id: 1, ok: false, kind: "Internal", fatal: true });
    // The reason is a fixed identifier from a closed set, never engine output (ADR 0009).
    expect(worker.posted[0]?.["message"]).toBe("no policy in force: probe-succeeded");
  });

  it("hands the qpdf glue the silencing options, so engine text cannot reach the console", () => {
    const worker = load(MAIN);
    worker.releaseQpdf();

    return worker.send(worker.op(1)).then(() => {
      expect(worker.qpdfOptions).toHaveLength(1);
      // ADR 0006 requirement 2. Dropping `...silent` from `init()` would otherwise be silent.
      expect(worker.qpdfOptions[0]?.printErr).toBeTypeOf("function");
      expect(worker.qpdfOptions[0]?.print).toBeTypeOf("function");
    });
  });

  // THE CASE THIS FILE EXISTS FOR. Everything above passes against a `main.js` that has never
  // been shown able to fail. This runs the same scenario against the revert spike 0003 named,
  // and asserts each case would go red.
  describe("and reverting the promise to a boolean flag is caught", () => {
    it("concurrent callers get two different promises", async () => {
      const worker = load(withBooleanFlag());

      const first = worker.ensureReady();
      const second = worker.ensureReady();
      // Weak on its own -- any `async function` returns a fresh promise, so this alone only
      // says `ensureReady` is async. The instance count below is the assertion that shows the
      // harm, and it is the counterpart to the real case's `toHaveLength(1)`.
      expect(second).not.toBe(first);

      worker.releaseQpdf();
      await Promise.all([first, second]);
      expect(worker.qpdfInstances).toHaveLength(2);
    });

    it("two concurrent operations instantiate qpdf TWICE, and attach the second over the first", async () => {
      const worker = load(withBooleanFlag());

      const both = Promise.all([worker.send(worker.op(1)), worker.send(worker.op(2))]);
      worker.releaseQpdf();
      await both;

      // The finding, reproduced: two independent instances, two attaches, and the bridge
      // left holding the later one while the earlier call was still in flight.
      expect(worker.qpdfInstances).toHaveLength(2);
      expect(worker.attaches).toHaveLength(2);
      expect(worker.attaches[1]?.qpdf).not.toBe(worker.attaches[0]?.qpdf);

      // Both operations still ran to completion, so the counts above are not the counts of a
      // path that fell over early for some unrelated reason.
      expect(worker.posted.filter((m) => m["ack"] === true)).toHaveLength(2);
      expect(worker.posted.filter((m) => m["ok"] === true)).toHaveLength(2);
    });
  });
});
