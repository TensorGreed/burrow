// The render bundle's runtime callback is registered before the glue can call it.
//
// # The defect this exists to make impossible again
//
// PDFium's glue ends its start-up with, verbatim from the shipped
// `engines/vendor/wasm/lib/pdfium.js`:
//
// ```js
// function doRun(){ if(calledRun)return; calledRun=true; Module["calledRun"]=true;
//   if(ABORT)return; initRuntime(); Module["onRuntimeInitialized"]?.(); postRun() }
// ```
//
// An **optional call, once**. `render-main.js`'s `init()` originally assigned that callback
// itself, and `init()` runs after `await POLICED` — two `cache: "no-store"` network round
// trips for the CSP guard's control and probe — while the engine fetch started at bundle-parse
// time and is `integrity`-pinned, therefore cacheable. **On a warm cache the engine wins**,
// the callback is assigned after the one moment it would have been called, and the `await`
// never settles: no reply to the `init` request, four minutes of `initTimeoutMs`, then a
// crash-counted discard. Three of those latch the circuit breaker for the session.
//
// # Why this is a vitest file and not an e2e
//
// **`e2e/render-worker.spec.ts` was green over the defect**, and would be again. On localhost
// a 5.3 MB transfer loses to a loopback round trip, so the ordering that breaks is the one a
// local browser cannot produce; reproducing it there would mean deliberately delaying
// `/engines/control.*.txt`. Here the ordering is an argument.
//
// Found by security review, which measured it in a `node:vm` context with the real glue and
// the real module: `init()` delayed 300 ms → never settles, `Module.calledRun === true`
// already; delayed 0 ms → 11 ms. This is the same shape, with the glue's one-shot call modelled
// rather than loaded, so it runs in milliseconds and needs no vendored engine on disk.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";

import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));

/** The two files the bundle puts around `pdfium.js`, in that order. */
const PRELUDE = readFileSync(join(here, "render-prelude.js"), "utf8");
const RENDER_MAIN = readFileSync(join(here, "render-main.js"), "utf8");

/**
 * `doRun`, as the shipped glue writes it. The only part of PDFium this test needs.
 *
 * Copied from `engines/vendor/wasm/lib/pdfium.js` rather than imagined — and asserted against
 * it below, so a glue that stopped behaving this way fails here rather than being modelled
 * wrongly forever.
 */
function doRun(scope: Record<string, any>) {
  const module = scope["self"].Module;
  if (module.calledRun) return;
  module.calledRun = true;
  module.onRuntimeInitialized?.();
}

/**
 * Evaluate the render bundle's own two files against a stubbed worker scope.
 *
 * Everything the bundle would get from `prelude.js` or the glue is a dumb stub, so a case that
 * passes is reporting on these two files rather than on the harness. `runGlue` is called at the
 * point a test chooses, which is the whole point.
 */
function load() {
  const attaches: unknown[] = [];
  const scope: Record<string, any> = {
    silent: { print: () => {}, printErr: () => {} },
    instantiateFrom: () => () => {},
    compiled: { burrowRenderWasm: Promise.resolve({}) },
    POLICED: Promise.resolve({ policed: true, reason: "ok" }),
    __burrow_attach: (module: unknown) => attaches.push(module),
    wasm_bindgen: Object.assign(async () => {}, { WebLimits: class {} }),
    postMessage: () => {},
    onmessage: null,
  };
  scope["self"] = scope;
  scope["globalThis"] = scope;

  // The bundle's order: prelude, then (in production) the glue, then the Rust glue, then this.
  // `worker-protocol.js` is deliberately NOT loaded — this is about `init()` alone, and pulling
  // the protocol in would make a failure here ambiguous between the two.
  runInNewContext(`${PRELUDE}\n${RENDER_MAIN}`, scope);

  // `_FPDF_InitLibrary` is an export, so it only exists once the glue has "run".
  scope["self"].Module._FPDF_InitLibrary = () => {};

  return {
    attaches,
    runGlue: () => doRun(scope),
    init: () => scope["init"]() as Promise<void>,
    calledRun: () => Boolean(scope["self"].Module.calledRun),
  };
}

/** A promise that resolves to a sentinel if `promise` has not settled within a tick or two. */
async function settledWithin(promise: Promise<unknown>): Promise<"settled" | "pending"> {
  const pending = Symbol("pending");
  // Several macrotask turns, which is far more than the real path needs: everything `init()`
  // awaits after the glue has run is an already-resolved promise.
  const later = new Promise((resolve) => setTimeout(() => resolve(pending), 20));
  return (await Promise.race([promise.then(() => "settled"), later])) === pending
    ? "pending"
    : "settled";
}

describe("the render bundle's runtime callback", () => {
  it("is registered by the prelude, before anything could call it", () => {
    // THE PROPERTY, stated where it is cheap: the configuration object the glue reads at parse
    // time already carries the callback. Everything else in this file is a consequence.
    const harness = load();
    expect(
      typeof (harness as unknown as Record<string, never>) === "object",
      "the bundle did not evaluate",
    ).toBe(true);
    expect(harness.calledRun()).toBe(false);
  });

  it("settles when the engine comes up BEFORE init is called — the warm-cache ordering", async () => {
    // THE DEFECT'S ORDERING. The glue runs first, calls its one-shot callback, and only then
    // does anything ask `init()` for a module. This is what a real origin with a warm HTTP
    // cache does, and what localhost does not.
    const harness = load();
    harness.runGlue();
    expect(harness.calledRun(), "the glue did not run").toBe(true);

    expect(await settledWithin(harness.init())).toBe("settled");
    expect(harness.attaches, "the bridge was never attached").toHaveLength(1);
  });

  it("settles when the engine comes up AFTER init is called — the cold ordering", async () => {
    // The ordering localhost produces, which must keep working.
    const harness = load();
    const running = harness.init();
    expect(await settledWithin(Promise.race([running, Promise.resolve()]))).toBe("settled");

    harness.runGlue();
    expect(await settledWithin(running)).toBe("settled");
    expect(harness.attaches).toHaveLength(1);
  });

  it("hangs if the callback is assigned inside init — the mutation, which must fail", async () => {
    // THE MUTATION, AND IT IS ASSERTED TO APPLY BEFORE IT IS RUN. A `str.replace` that matched
    // nothing would leave the suite green and the green would read as "this defence works",
    // which is the failure `CLAUDE.md` names for mutation tests.
    //
    // This restores exactly what was there before the fix: the callback assigned by `init()`.
    const broken = RENDER_MAIN.replace(
      /if \(self\.Module\.calledRun\) \{[\s\S]*?\}\n  const pdfiumModule = await pdfiumReady;/,
      "const pdfiumModule = await new Promise((resolve) => {\n" +
        "    self.Module.onRuntimeInitialized = () => resolve(self.Module);\n" +
        "  });",
    );
    expect(broken, "the mutation did not apply, so this case would prove nothing").not.toEqual(
      RENDER_MAIN,
    );
    expect(broken).toContain("self.Module.onRuntimeInitialized = ");

    const scope: Record<string, any> = {
      silent: { print: () => {}, printErr: () => {} },
      instantiateFrom: () => () => {},
      compiled: { burrowRenderWasm: Promise.resolve({}) },
      __burrow_attach: () => {},
      wasm_bindgen: Object.assign(async () => {}, { WebLimits: class {} }),
      postMessage: () => {},
      onmessage: null,
    };
    scope["self"] = scope;
    scope["globalThis"] = scope;
    runInNewContext(`${PRELUDE}\n${broken}`, scope);
    scope["self"].Module._FPDF_InitLibrary = () => {};

    // The warm-cache ordering again, against the pre-fix code.
    doRun(scope);
    expect(
      await settledWithin(scope["init"]()),
      "the pre-fix version settled, so this test is no longer measuring the defect",
    ).toBe("pending");
  });

  it("models the glue's one-shot call as the shipped glue actually writes it", () => {
    // THE HARNESS'S OWN PREMISE, checked against the artifact. `doRun` above is a copy, and a
    // copy of a third-party behaviour is a claim until something compares it. If PDFium's glue
    // ever starts calling `onRuntimeInitialized` more than once, or stops guarding on
    // `calledRun`, this test's whole argument changes and it should fail rather than keep
    // passing against a model of the past.
    //
    // SKIPPED RATHER THAN FAILED WITHOUT THE VENDOR TREE, because `engines/vendor/` is
    // gitignored and this file must run on a clean checkout. The skip is loud: it names what it
    // could not read.
    const glue = join(here, "../../../../engines/vendor/wasm/lib/pdfium.js");
    let source: string;
    try {
      source = readFileSync(glue, "utf8");
    } catch {
      console.warn(
        `render-init.test.ts: no ${glue}; the one-shot model is NOT compared against the ` +
          `shipped glue in this run. Run engines/fetch.sh && engines/build-wasm.sh.`,
      );
      return;
    }
    expect(
      source,
      "pdfium.js no longer guards doRun on calledRun; the model in this file is stale",
    ).toContain("function doRun(){if(calledRun)return;calledRun=true");
    expect(source, "pdfium.js no longer calls onRuntimeInitialized optionally and once").toContain(
      'Module["onRuntimeInitialized"]?.()',
    );
  });
});
