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

"use strict";

/**
 * Whether this worker was created from a Blob, and therefore inherits the page's CSP.
 *
 * **The fail-closed guard.** If this bundle is ever constructed from a plain URL — by a
 * future refactor, or by a page that has not been updated — it would run unpoliced, and
 * every CSP test would still pass because they exercise the blob path. Refusing to accept
 * files unless the protocol says otherwise means that mistake is loud rather than silent.
 *
 * `self.location` in a Blob worker is the `blob:` URL it was constructed from.
 */
const INHERITS_PAGE_CSP = self.location.protocol === "blob:";

/** Silence both modules at the JavaScript layer. ROADMAP item 7's web half. */
const silent = { print: () => {}, printErr: () => {} };

// Kick the engine fetches off immediately: they are the long pole, and the glue below will
// wait on `instantiateWasm` regardless of how long they take.
//
// `BURROW_ENGINES` is generated into the bundle above this file by
// tools/stage-web-engines.mjs.
/** @type {Record<string, Promise<WebAssembly.Module>>} */
const compiled = {};
for (const id of ["pdfiumWasm", "qpdfWasm", "burrowWasm"]) {
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
