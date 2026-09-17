// The render bundle's third file: everything `prelude.js` cannot hold because it is shared.
//
// Concatenated between `prelude.js` and `pdfium.js`, and that position is the whole reason it
// is a separate file rather than a branch. `pdfium.js` reads a pre-existing global `Module`
// for its configuration **at load time** — it begins instantiating as it is parsed — so
// everything below has to have already run, and there is no moment after the glue loads at
// which anything could arrive by `postMessage` instead. ADR 0014 §1a records the same
// constraint for the engine manifest, which is generated into the bundle for the same reason.
//
// `silent` and `instantiateFrom` come from `prelude.js`. `BURROW_ENGINES` is generated above
// it. Nothing here is defined twice; this file is the difference between the two bundles and
// nothing else, which is what keeps `prelude.js` byte-identical in both — the property that
// makes the CSP guard, the fail-closed refusal and the progress protocol one implementation
// rather than two that drifted.

/**
 * Resolves when PDFium's runtime is up. `render-main.js`'s `init()` awaits this.
 *
 * # `onRuntimeInitialized` IS ONE-SHOT, AND THIS IS WHY THE PROMISE IS CREATED HERE
 *
 * The shipped glue ends its start-up with, verbatim:
 *
 * ```js
 * function doRun(){ if(calledRun)return; calledRun=true; Module["calledRun"]=true;
 *   if(ABORT)return; initRuntime(); Module["onRuntimeInitialized"]?.(); postRun() }
 * ```
 *
 * An **optional call, once**. A callback assigned after that moment is never invoked, and the
 * `await` waiting on it never settles.
 *
 * `init()` cannot assign it in time, and the reason is a race rather than an oversight. It is
 * reached from `worker-protocol.js`'s `ensureReady()`, which runs **after** `await POLICED` —
 * two `cache: "no-store"` network round trips for the guard's control and probe. Meanwhile
 * `prelude.js` started fetching `pdfium.wasm` when the bundle was parsed, `integrity`-pinned
 * and therefore cacheable, and `instantiateFrom` calls Emscripten's `done()` the moment it
 * lands. **On a warm HTTP cache the engine wins that race**; compiling the 5.3 MB module
 * measures ~10 ms, against a network round trip for the control.
 *
 * SECURITY REVIEW MEASURED IT, in a `node:vm` context with the real glue and the real module:
 * with `init()` delayed 300 ms the promise **never settles** and `Module.calledRun` is already
 * `true`. With no delay it resolves in 11 ms. **Localhost is the only place this works**,
 * because there a 5.3 MB transfer loses to a loopback round trip — which is exactly why
 * `e2e/render-worker.spec.ts` was green over the defect.
 *
 * The consequence was not a typed refusal but a hang: no reply to the `init` request, four
 * minutes of `initTimeoutMs` re-armed by each `starting` message, then a **crash-counted**
 * discard — and three of those latch the circuit breaker for the session.
 *
 * So the callback is part of the configuration object the glue reads, which is the only
 * placement that cannot lose the race.
 */
/** @type {(module: EmscriptenModule) => void} */
let resolvePdfium;
/** @type {Promise<EmscriptenModule>} */
const pdfiumReady = new Promise((resolve) => {
  resolvePdfium = resolve;
});

// PDFium's glue reads this at load time. It must exist before the next file in the bundle.
//
// `...silent` is ROADMAP item 7's web half and is not optional here: PDFium's prebuilt glue
// is not built with `-sENVIRONMENT=web,worker` (we unpack it verbatim), so it carries every
// default path including the ones that write to the console. `e2e/render-worker.spec.ts`
// covers this bundle for the same reason it covers the other one.
self.Module = {
  ...silent,
  instantiateWasm: instantiateFrom("pdfiumWasm"),
  // THE NARROWING `EmscriptenConfig` DOCUMENTS, at the one point where it actually holds.
  // The glue populates this same object with its exports and then calls this; before that
  // there is no `_malloc` on it, which is why the two types are separate. Here, and only
  // here, it is a running module.
  onRuntimeInitialized: () =>
    resolvePdfium(/** @type {EmscriptenModule} */ (/** @type {unknown} */ (self.Module))),
};
