// The first thing that runs in the worker. Bundled ahead of the Emscripten glue.
//
// WHY THE WORKER IS ONE BUNDLED FILE, LOADED FROM A blob:
//
// Measured during M1 PR 4a-i, and it invalidated the design that preceded it: **a dedicated
// worker created from a same-origin script URL does not inherit the creating document's
// Content-Security-Policy.** It takes its policy from that script's own HTTP response
// headers, and a static host sends none — so the worker ran with *no policy at all*, and a
// cross-origin `fetch` from inside it reached the network. The page-level CSP tests passed
// throughout and proved nothing about the worker, which is the only place file bytes ever
// exist.
//
// Only `blob:`, `data:` and `about:` workers inherit. So the page fetches this bundle's
// source text — with `integrity`, from a `connect-src` entry naming its exact URL — wraps it
// in a Blob, and constructs the worker from that. The document's policy then applies here.
//
// Bundling everything into one file falls out of the same change, and fixes a second
// problem: `importScripts` has no integrity mechanism, so the three glue files (including
// 160 KB of third-party PDFium glue that runs with full worker privileges and owns the heap
// this bridge writes to) were previously loaded unverified. One file means one integrity
// check covering *all* worker code, and it means `pdfium.js` — which is not modularised and
// whose state lives in worker globals — is loaded exactly once by construction.
//
// ONE OF THE TWO MODULES IS NOT OURS, AND THE COMMENTS SHOULD NOT PRETEND OTHERWISE
//
// burrow builds `qpdf.js` with `-sENVIRONMENT=web,worker`, which removes its Node branches
// outright. `pdfium.js` is a prebuilt, unpacked verbatim: it still contains
// `ENVIRONMENT_IS_NODE`, `require("fs")`, a synchronous `XMLHttpRequest` path and a
// `fetch(binaryFile, ...)`. None of it runs -- `instantiateWasm` pre-empts the fetch and a
// browser worker is not Node -- but that is established by the CSP and by 4a-ii's
// zero-requests test, NOT by a build flag we never applied to it.
//
// See docs/adr/0014-web-engine-loading-and-csp.md.
//
// ORDER MATTERS IN THIS BUNDLE
//
// `pdfium.js` reads a pre-existing global `Module` for its configuration *at load time*, so
// `self.Module` must be assigned before it runs. That is this file's main job. The engine
// manifest is generated into the bundle above this point rather than arriving by
// `postMessage`, because the glue starts instantiating as soon as it is parsed — there is no
// moment after load and before instantiation at which a message could be delivered.

// NO "use strict" HERE, deliberately.
//
// It was here and it was INERT: the bundle emits the generated `BURROW_ENGINES` const before
// this file, so the directive is no longer in a directive prologue and has no effect on any
// of the bundle's ~1,000 lines. Leaving it in would be a comment that claims a guarantee the
// code does not have.
//
// Making it real would mean emitting it as the bundle's genuine first statement, which would
// also flip 160 KB of third-party Emscripten glue to strict mode -- a much larger and
// entirely untested change for no benefit we need. What strict mode would buy here is a
// `ReferenceError` on an undeclared assignment, and `tsc -p src/worker` already reports that
// as "Cannot find name" (verified). `src/production-build.test.ts` asserts the bundle's mode
// so this cannot drift back silently.

/**
 * The path the policy probe requests.
 *
 * Same-origin and not a real route, so that if the policy were somehow absent the request
 * is a 404 on our own host rather than a request to anyone else. The leading `__` marks it
 * as not-a-page; nothing serves it.
 */
const PROBE_PATH = "/__csp-probe";

/**
 * How long to wait for `securitypolicyviolation` before concluding there is no policy.
 *
 * The event is dispatched on the event loop, so it lands within a turn or two of the fetch
 * rejecting; this is generous. It bounds a guard that must never hang: exceeding it resolves
 * *false*, and the worker then refuses to touch a file.
 */
const PROBE_TIMEOUT_MS = 250;

/**
 * Whether this worker was created from a Blob.
 *
 * Necessary but not sufficient: a Blob worker inherits the creating document's policy —
 * *whatever that is*, including none. A page that omitted the layout, or a host that served
 * a malformed meta tag, produces a `blob:` worker with an empty policy. So this is the cheap
 * structural check and {@link POLICED} is the one that actually decides.
 *
 * `self.location` in a Blob worker is the `blob:` URL it was constructed from.
 */
const CREATED_FROM_BLOB = self.location.protocol === "blob:";

/**
 * Whether a Content-Security-Policy is actually in force in *this* worker.
 *
 * **The fail-closed guard, and it measures the property rather than a proxy for it.**
 *
 * An earlier version checked only {@link CREATED_FROM_BLOB} and called itself
 * `INHERITS_PAGE_CSP` — which asserts a stronger thing than it tested. This attempts a fetch
 * to a same-origin URL that is deliberately *not* in `connect-src` and requires the browser
 * to refuse it, listening for `securitypolicyviolation` so that "refused by policy" is
 * distinguishable from "the request failed". A network error is not proof of a policy.
 *
 * The probe runs once, before any file can arrive, and is itself blocked — so it sends
 * nothing. Its cost is one refused request at init.
 */
const POLICED = (async () => {
  if (!CREATED_FROM_BLOB) {
    return false;
  }

  /**
   * Resolves true when the browser reports a policy violation, false if none arrives.
   *
   * Bounded: a guard that waited indefinitely for an event that may never come would hang
   * the worker on its first message rather than refuse, which is a worse failure than the
   * one it is preventing. The timeout resolves **false**, so the whole check fails closed.
   */
  const violation = new Promise((resolve) => {
    const timer = setTimeout(() => {
      self.removeEventListener("securitypolicyviolation", onViolation);
      resolve(false);
    }, PROBE_TIMEOUT_MS);
    function onViolation() {
      clearTimeout(timer);
      self.removeEventListener("securitypolicyviolation", onViolation);
      resolve(true);
    }
    self.addEventListener("securitypolicyviolation", onViolation);
  });

  try {
    // A same-origin path that is deliberately not in `connect-src`, and deliberately not a
    // real route. NEVER a cross-origin URL: if the policy were missing, a cross-origin probe
    // would actually reach a third party -- a guard whose failure mode is the thing it
    // guards against. With `/__csp-probe` the worst case is a 404 on our own host.
    //
    // Absolute, because a blob: worker's `self.location` is opaque and cannot resolve a
    // relative reference.
    await fetch(new URL(PROBE_PATH, BURROW_ENGINES.probeOrigin).href, { mode: "no-cors" });
    // The request was NOT refused, so there is no policy in force here.
    return false;
  } catch {
    // A rejected fetch is not enough on its own: a network error rejects too. Only the
    // violation event says the browser refused it, and it is dispatched asynchronously --
    // reading a flag straight after the rejection reports false for a request the policy
    // did refuse. (It did, and every engine test went red.)
    return violation;
  }
})();

/** Silence both modules at the JavaScript layer. ROADMAP item 7's web half. */
const silent = { print: () => {}, printErr: () => {} };

// Kick the engine fetches off immediately: they are the long pole, and the glue below will
// wait on `instantiateWasm` regardless of how long they take.
//
// `BURROW_ENGINES` is generated into the bundle above this file by
// tools/stage-web-engines.mjs.
/** @type {Record<string, Promise<WebAssembly.Module>>} */
const compiled = {};
for (const id of /** @type {const} */ (["pdfiumWasm", "qpdfWasm", "burrowWasm"])) {
  const entry = BURROW_ENGINES[id];
  compiled[id] = fetch(entry.url, { integrity: entry.integrity }).then((response) => {
    if (!response.ok) {
      throw new Error("engine fetch failed");
    }
    return WebAssembly.compileStreaming(response);
  });
}

/**
 * Hand an already-compiled module to Emscripten instead of letting it fetch or locate one.
 *
 * Returning `{}` tells Emscripten the instantiation is asynchronous and that `done` will be
 * called later — which is what lets the fetch above still be in flight when the glue is
 * parsed.
 *
 * This is also what makes a `blob:` worker safe for Emscripten at all: `locateFile` resolves
 * relative to `self.location`, which under a Blob is an opaque `blob:` URL with no useful
 * base. Supplying the module means that path never runs.
 *
 * @param {string} id
 */
function instantiateFrom(id) {
  return (
    /** @type {WebAssembly.Imports} */ imports,
    /** @type {(instance: WebAssembly.Instance, module: unknown) => void} */ done,
  ) => {
    compiled[id]
      .then((module) => WebAssembly.instantiate(module, imports))
      .then((instance) => done(instance, compiled[id]));
    return {};
  };
}

// PDFium's glue reads this at load time. It must exist before the next file in the bundle.
self.Module = {
  ...silent,
  instantiateWasm: instantiateFrom("pdfiumWasm"),
};
