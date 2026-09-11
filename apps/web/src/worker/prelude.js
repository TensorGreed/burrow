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
 * How long to wait for the probe request to settle before concluding nothing was refused.
 *
 * A refusal is immediate — the browser never sends the request — so this is generous. It
 * bounds a guard that must never hang: exceeding it resolves *false*, and the worker then
 * refuses to touch a file.
 */
const PROBE_TIMEOUT_MS = 5_000;

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

/**
 * Whether a Content-Security-Policy is actually in force in *this* worker, and why.
 *
 * **The fail-closed guard, and it measures the property rather than a proxy for it.**
 *
 * Two earlier versions were wrong, and both are worth knowing about:
 *
 *   * `self.location.protocol === "blob:"` alone, named `INHERITS_PAGE_CSP`. A Blob worker
 *     inherits the creating document's policy *whatever that is, including none*, so the name
 *     asserted more than the check established.
 *   * Requiring a `securitypolicyviolation` event. **WebKit does not dispatch one in a
 *     worker**: it refused the probe, fired nothing, and the guard concluded there was no
 *     policy and refused every operation in a browser that was enforcing it correctly.
 *
 * So it is **differential**, using only whether requests succeed — which every browser agrees
 * on. An allowlisted request must succeed (the control) and a non-allowlisted one must be
 * refused (the probe).
 *
 * **Both are issued together and both are `cache: "no-store"`**, so they face identical
 * network conditions. The control is a dedicated few-byte resource rather than one of the
 * engine fetches: an engine response can come from the HTTP cache, so offline-with-a-warm-
 * cache would let the control succeed while the probe failed for network reasons, and the
 * guard would report "policed" with nothing enforcing anything.
 *
 * Resolves a `{ policed, reason }` verdict. `reason` is reported to the page on refusal, by
 * message — never to the console, which is where file-derived bytes must never go and which
 * nobody reads in production anyway.
 */
const POLICED = (async () => {
  if (!CREATED_FROM_BLOB) {
    return { policed: false, reason: "not-a-blob-worker" };
  }

  /** @param {string} path */
  const absolute = (path) => new URL(path, BURROW_ENGINES.probeOrigin).href;

  /**
   * Run one request and classify the outcome, without swallowing anything unexpected.
   *
   * A `fetch` that is refused by the policy, or that fails at the network level, rejects with
   * a `TypeError`. That is the ONLY rejection this is prepared to interpret. Anything else —
   * a `ReferenceError` from a future edit, say — is a bug in the guard, not evidence about
   * the policy, and must not be quietly read as either answer.
   *
   * A bare `catch { return false }` did swallow exactly that: `POLICED` once referenced a
   * binding declared below it, hit the temporal dead zone, and reported "not policed" for a
   * programming error. Fail-closed, so not dangerous, but silent and wrong.
   */
  /** @param {string} url @returns {Promise<"succeeded" | "rejected">} */
  const attempt = async (url) => {
    try {
      await fetch(url, { mode: "no-cors", cache: "no-store" });
      return "succeeded";
    } catch (error) {
      // `name`, not `instanceof`. `instanceof` compares against THIS realm's constructor, and
      // an error that crossed a realm boundary fails it even when it is a TypeError -- which
      // is not hypothetical: the unit tests drive this code in a `vm` context, and every
      // rejection they stage was classified `guard-error` until this changed. A worker is one
      // realm in production, but a check that depends on that is a check that only works
      // where it is not tested.
      // Structural, with no `instanceof` at all -- `instanceof Object` fails across realms
      // for exactly the same reason `instanceof TypeError` does.
      if (
        typeof error === "object" &&
        error !== null &&
        /** @type {{ name?: unknown }} */ (error).name === "TypeError"
      ) {
        return "rejected";
      }
      throw error;
    }
  };

  /** Bounded, so a request that neither resolves nor rejects cannot hang the worker. */
  const timeout = new Promise((resolve) => {
    setTimeout(() => resolve("timed-out"), PROBE_TIMEOUT_MS);
  });

  try {
    // Issued together, deliberately. Sequencing them would let conditions change in between,
    // which is the whole thing the control exists to rule out.
    const [control, probe] = await Promise.all([
      Promise.race([attempt(absolute(BURROW_ENGINES.control.url)), timeout]),
      Promise.race([attempt(absolute(PROBE_PATH)), timeout]),
    ]);

    if (control !== "succeeded") {
      // The network is unreachable, or the control resource is missing. A refused probe
      // proves nothing in that state, so there is nothing to conclude.
      return { policed: false, reason: `control-${control}` };
    }
    if (probe === "rejected") {
      return { policed: true, reason: "ok" };
    }
    // `fetch` resolves on a 404, so a probe that merely 404s lands here: something served it,
    // which means nothing refused it.
    return { policed: false, reason: `probe-${probe}` };
  } catch {
    // Anything `attempt` re-threw. Deliberately not inspected or logged: it can only be a bug
    // in this file, and its text is not ours to forward. Distinct from every outcome above so
    // the page can tell a broken guard from an absent policy.
    return { policed: false, reason: "guard-error" };
  }
})();

// PDFium's glue reads this at load time. It must exist before the next file in the bundle.
self.Module = {
  ...silent,
  instantiateWasm: instantiateFrom("pdfiumWasm"),
};
