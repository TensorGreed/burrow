// The test-only harness driver: everything a browser test needs, over the real host.
//
// NOT SHIPPED. `astro.config.mjs`'s `burrow:harness-gating` deletes `/host/` from `dist/`
// unless `BURROW_HARNESS=1`, and `src/production-build.test.ts` asserts it is gone — with a
// control that builds WITH the flag, so an integration that excluded unconditionally would
// fail rather than look perfect.
//
// WHAT IS HERE AND WHAT IS IN `worker-host.js`
//
// Every lifecycle decision — when a worker dies, when one is spawned, what the watchdog does,
// when the breaker opens — is in `worker-host.js`, which knows nothing about the DOM, `fetch`,
// `Worker` or `performance`. This file supplies those four things and nothing else, plus the
// test affordances below.
//
// That split is what lets `worker-host.test.ts` drive every state in milliseconds against a
// fake worker, including the ones a browser will not reproduce on demand.
//
// THE TEST AFFORDANCES, AND WHY THEY ARE HONEST
//
// Three browser tests need the worker to misbehave on cue: a deliberate trap (ADR 0009's
// required recovery test), a hang the watchdog must kill, and a console write the
// console-silence test must be able to see.
//
// None of them is a hook in production code. The page fetches the worker's source text with
// `integrity` and wraps it in a Blob; this file prepends a **prologue** to that already-
// verified text. The prologue is test code, in a test-only file, acting on bytes whose digest
// was already checked.
//
// The trap affordance is the one worth defending. It replaces one `__burrow_*` bridge global
// with a function that throws. A normal `page_count` then runs through the real Rust, which
// calls the bridge, and a real JavaScript exception propagates back out through the wasm
// frames — which is exactly the failure ADR 0009 describes as the sneaky one: "an Emscripten
// `abort()` inside an engine module throws a JS exception, which becomes an ordinary
// `Error::Internal` reply". It is not a simulation of that path; it is that path.

import { createWorkerHost } from "./worker-host.js";

/**
 * @typedef {object} EngineEntry
 * @property {string} url
 * @property {string} integrity
 */

/**
 * One element the harness page is required to have.
 *
 * A hard failure rather than an optional chain: if the page and this file disagree about the
 * markup, every test would otherwise fail later with something less informative.
 *
 * @param {string} id
 * @returns {HTMLElement}
 */
function required(id) {
  const element = document.getElementById(id);
  if (!element) {
    throw new Error(`the harness page is missing #${id}`);
  }
  return element;
}

const status = required("status");
const result = required("result");
const ENGINES = /** @type {Record<string, EngineEntry>} */ (
  JSON.parse(required("engines").dataset.engines ?? "{}")
);

/** The default ceilings a browser test runs under, unless it says otherwise. */
const DEFAULT_LIMITS = {
  maxInputBytes: 100 * 1024 * 1024,
  maxMemoryBytes: 1024 * 1024 * 1024,
  maxDurationMs: 30000,
  maxPages: 10000,
  maxPixels: 100000000,
};

// --- the worker source, fetched once and pinned ---------------------------------------

/** @type {string | null} */
let workerSource = null;
/** @type {string | null} */
let workerUrl = null;

/** The prologue the next spawn will carry. Rebuilt whenever an affordance is armed. */
let prologue = "";

/** Console lines the worker emitted, captured by the prologue. @type {string[]} */
const workerConsole = [];

/** One file, held across operations. See `holdFile`. @type {File | null} */
let held = null;

async function ensureSource() {
  if (workerSource === null) {
    const entry = ENGINES.worker;
    const response = await fetch(entry.url, { integrity: entry.integrity });
    if (!response.ok) {
      throw new Error("worker fetch failed");
    }
    workerSource = await response.text();
  }
  return workerSource;
}

/**
 * Build the object URL the next worker is constructed from.
 *
 * A worker MUST be created from a Blob. One loaded from a plain same-origin URL does not
 * inherit this page's CSP — it takes its policy from that script's HTTP response headers, and
 * a static host sends none, so it would run unpoliced. Measured in M1 PR 4a-i; see ADR 0014.
 * The worker itself refuses to touch a file unless a policy is actually in force, so getting
 * this wrong fails closed rather than silently dropping the browser-enforced half.
 */
/** @param {string} source */
function buildUrl(source) {
  if (workerUrl) {
    URL.revokeObjectURL(workerUrl);
  }
  workerUrl = URL.createObjectURL(new Blob([prologue, source], { type: "text/javascript" }));
  return workerUrl;
}

// --- the host --------------------------------------------------------------------------

/**
 * The prologue's channel back to the page.
 *
 * A separate `onmessage` would fight the host for the worker's message port, so the prologue
 * tags its own messages and the host ignores anything it does not recognise. This reads them
 * off the same worker object before handing it over.
 */
/**
 * @param {Worker} worker
 * @returns {Worker}
 */
function instrument(worker) {
  /** @type {{ current: ((event: { data: any }) => void) | null }} */
  const hostHandler = { current: null };
  Object.defineProperty(worker, "onmessage", {
    configurable: true,
    get: () => hostHandler.current,
    set: (handler) => {
      hostHandler.current = handler;
    },
  });
  worker.addEventListener("message", (/** @type {MessageEvent} */ event) => {
    if (event.data?.__burrowConsole) {
      workerConsole.push(String(event.data.__burrowConsole));
      return;
    }
    hostHandler.current?.(event);
  });
  return worker;
}

let sourceForSpawn = "";

const host = createWorkerHost({
  spawn: () => instrument(new Worker(buildUrl(sourceForSpawn))),
  release: () => {
    if (workerUrl) {
      URL.revokeObjectURL(workerUrl);
      workerUrl = null;
    }
  },
  now: () => performance.now(),
  setTimer: (fn, ms) => setTimeout(fn, ms),
  clearTimer: (handle) => clearTimeout(/** @type {number} */ (handle)),
  maxDurationMs: DEFAULT_LIMITS.maxDurationMs,
});

/**
 * The prologue source for the currently armed affordances.
 *
 * Rebuilt rather than appended so arming is idempotent: a test that arms twice gets one
 * prologue, not two copies fighting over the same global.
 *
 * @param {{ captureConsole?: boolean, logCanary?: string | null, poison?: string | null, hangMs?: number }} options
 */
function buildPrologue({ captureConsole = false, logCanary = null, poison = null, hangMs = 0 }) {
  const parts = [];
  if (captureConsole) {
    // EVERY method, not just `log`. Playwright does not deliver worker console messages
    // uniformly across the three browsers, so the only way to see what the real bundle writes
    // to a worker console in WebKit is to be the console.
    parts.push(`
      for (const level of ["log", "info", "warn", "error", "debug", "trace", "dir", "table"]) {
        const original = console[level];
        console[level] = (...args) => {
          self.postMessage({ __burrowConsole: level + ": " + args.join(" ") });
          if (original) original.apply(console, args);
        };
      }`);
  }
  if (logCanary) {
    // THE CONTROL. Without it a broken capture leaves every case green, which converts
    // "nobody checked" into "something checked and it was fine" — the failure mode
    // `core/burrow-engines/tests/secret_leak.rs` exists to avoid.
    parts.push(`console.warn(${JSON.stringify(logCanary)});`);
  }
  if (poison) {
    // A real exception out of the wasm module. See the header: Rust calls this bridge global,
    // and the throw propagates back through the wasm frames exactly as an Emscripten abort()
    // inside an engine does.
    //
    // Installed on the first message rather than at parse time, because the bundle defines
    // these globals as it is parsed and this file runs before it. `once` so a second message
    // does not wrap a wrapper.
    parts.push(`
      self.addEventListener("message", () => {
        self[${JSON.stringify(poison)}] = () => {
          throw new Error("deliberate engine failure");
        };
      }, { once: true });`);
  }
  if (hangMs > 0) {
    // A SYNCHRONOUS block, deliberately. An `await` would leave the worker responsive and
    // prove nothing: what the watchdog exists for is a single engine call that never returns
    // control, which ADR 0007 says checkpoint-based enforcement cannot interrupt.
    // WRAPS `__burrow_qpdf_copy_in`, which it did not until spike 0004 --- it wrapped
    // `__burrow_pdfium_copy_in`, and PDFium is no longer in the web payload, so that global
    // does not exist. A wrapper over `undefined` throws on the FIRST operation instead of
    // hanging on it, which the page reports as `Internal`: the watchdog test would have gone
    // on passing while measuring a crash rather than a hang.
    //
    // `copy_in` is the right hook for the same reason it was before: it is the first engine
    // call every operation makes, so the hang lands inside the operation rather than before
    // it starts.
    parts.push(`
      self.addEventListener("message", () => {
        const original = self.__burrow_qpdf_copy_in;
        self.__burrow_qpdf_copy_in = (bytes) => {
          const until = Date.now() + ${hangMs};
          while (Date.now() < until) {}
          return original(bytes);
        };
      }, { once: true });`);
  }
  return parts.length > 0 ? `(() => {${parts.join("\n")}})();\n` : "";
}

// --- the API the Playwright tests drive ------------------------------------------------

/**
 * A reply Playwright can carry back out of the page.
 *
 * `reply.output` is a **Blob**, which `page.evaluate` cannot structured-clone to Node: it
 * would arrive as `{}` and every assertion about it would be vacuously true. The harness
 * reports its LENGTH instead, which is all a conformance run needs -- the outcome it
 * records is the page count, and the bytes themselves are B3's business.
 *
 * The Blob is dropped here rather than held, so the page is not keeping a merged document
 * alive for the rest of the run.
 */
/**
 * @param {import("./harness-api.js").Reply & { output?: Blob | null }} reply
 * @returns {import("./harness-api.js").Reply}
 */
/**
 * @param {import("./worker-host.js").HostReply & { output?: Blob | null, parts?: Blob[] }} reply
 */
function serialisable(reply) {
  // `parts` COMES OUT TOO, and it was not until code review pointed out the fourth reader.
  // `output` was stripped because a `Blob` is not structured-cloneable across the Playwright
  // boundary; `parts` is an array of them and arrived later, through `worker-host.js`'s
  // multi-output path. A `page.evaluate` returning one resolves to `undefined`, which would
  // make the conformance spec throw on `parts.fatal` -- a failure with nothing to do with
  // split. `add-operation` §2a's "three readers move together" has a fourth here.
  const { output, parts, ...rest } = reply;
  return {
    ...rest,
    outputBytes: output ? output.size : 0,
    partCount: parts ? parts.length : 0,
  };
}

/**
 * Run one operation over a `Blob`, returning the RAW reply.
 *
 * Extracted from `runBase64` when `rotateEveryPage` needed the reply's `output` Blob to feed
 * into a second call: `serialisable` replaces `output` with its size, which is right for
 * crossing back into a Playwright test and useless for chaining inside the driver.
 *
 * @param {string} op
 * @param {Blob} blob
 * @param {{ password?: string | null, limits?: Record<string, number>, extra?: string[],
 *           attemptRecovery?: boolean, pages?: number[], degrees?: number,
 *           order?: number[], cuts?: number[] }} options
 */
async function runOnBlob(op, blob, options = {}) {
  sourceForSpawn = await ensureSource();
  // ON THE CORE'S DEFAULTS, not the page's. `expectations.json` says an omitted `limits`
  // block means `Limits::DEFAULT`, and the native side honours that literally -- so merging
  // a case's overrides onto `DEFAULT_LIMITS` (100 MiB of `maxInputBytes` against the core's
  // 512 MiB) would mean the two sides of the differential harness ran under different
  // ceilings. No fixture is large enough for it to bite today, which is exactly why it would
  // have gone unnoticed.
  const base = host.coreDefaultLimits() ?? DEFAULT_LIMITS;
  const limits = { ...base, ...(options.limits ?? {}) };
  const password = options.password ? new TextEncoder().encode(options.password).buffer : null;
  // MERGE TAKES A LIST. `blob` is one document for every other operation; for merge the
  // caller passes `options.extra`, the remaining documents in order, and this assembles the
  // list. Kept as a separate field rather than always sending a list, so a single-input
  // operation's message shape is unchanged.
  const extra = (options.extra ?? []).map((b64) => {
    const raw = atob(b64);
    const out = new Uint8Array(raw.length);
    for (let i = 0; i < raw.length; i += 1) out[i] = raw.charCodeAt(i);
    return new Blob([out], { type: "application/pdf" });
  });
  return host.run(
    {
      op,
      blob,
      blobs: op === "merge" ? [blob, ...extra] : undefined,
      // `rotate` only. One-based page numbers and a quarter turn, both chosen by the caller
      // and neither derived from the document.
      pages: options.pages,
      degrees: options.degrees,
      // `reorder` only. A permutation of one-based page numbers, chosen by the caller.
      order: options.order,
      // `split` only. One-based page numbers to cut after, chosen by the caller.
      cuts: options.cuts,
      password,
      limits,
      attemptRecovery: options.attemptRecovery ?? false,
    },
    { maxDurationMs: limits.maxDurationMs },
  );
}

/** @type {import("./harness-api.js").BurrowHarness} */
const harness = {
  async ready() {
    sourceForSpawn = await ensureSource();
    return host.ready();
  },

  /**
   * Run one operation over `bytes`, a plain array of byte values from the test.
   *
   * The bytes become a **Blob**, which is what the worker receives — not a transferred
   * `ArrayBuffer`. Structured clone passes a Blob by reference, so the page never materialises
   * the file in its own heap and, more usefully here, the caller can still run the same file
   * again after a worker is killed. A transferred buffer is detached page-side and gone.
   */
  async run(op, bytes, options = {}) {
    sourceForSpawn = await ensureSource();
    const limits = { ...DEFAULT_LIMITS, ...(options.limits ?? {}) };
    const blob = new Blob([new Uint8Array(bytes)], { type: "application/pdf" });
    const password = options.password ? new Uint8Array(options.password).buffer : null;
    return host.run(
      {
        op,
        blob,
        password,
        limits,
        attemptRecovery: options.attemptRecovery ?? false,
      },
      { maxDurationMs: limits.maxDurationMs },
    );
  },

  /**
   * Keep one `File` in page scope, and run operations against THAT object.
   *
   * The only way to test what the Blob decision actually buys. `run()` builds a fresh Blob per
   * call, so a test using it would pass identically against an implementation that transferred
   * a detachable `ArrayBuffer` — nothing is reused, so nothing can be detached. Holding one
   * handle and running it twice across a worker death is the claim.
   */
  holdFile(bytes) {
    held = new File([new Uint8Array(bytes)], "held.pdf", { type: "application/pdf" });
  },

  /** Run an operation against the held file. See {@link holdFile}. */
  async runHeld(op, options = {}) {
    if (!held) {
      throw new Error("no file is held");
    }
    sourceForSpawn = await ensureSource();
    const limits = { ...DEFAULT_LIMITS, ...(options.limits ?? {}) };
    return host.run(
      { op, blob: held, password: null, limits, attemptRecovery: false },
      { maxDurationMs: limits.maxDurationMs },
    );
  },

  /** Whether the held file is still readable — i.e. was not detached by a transfer. */
  async heldIsStillReadable() {
    if (!held) {
      return false;
    }
    return (await held.arrayBuffer()).byteLength > 0;
  },

  /**
   * Run one operation over base64-encoded bytes.
   *
   * The conformance corpus runs 21 cases through 2 operations in 3 browsers, and the biggest
   * fixture is 330 KB. As a `number[]` that is a ~1.3 MB JSON payload per `page.evaluate`;
   * base64 is a third of that and decodes in one call. `run()` keeps taking an array, because
   * every other spec is more readable that way and their fixtures are tiny.
   */
  async runBase64(op, base64, options = {}) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      bytes[i] = binary.charCodeAt(i);
    }
    const blob = new Blob([bytes], { type: "application/pdf" });
    return serialisable(await runOnBlob(op, blob, options));
  },

  /**
   * Rotate every page by 90 and report the rotations of the result.
   *
   * THREE OPERATIONS, MIRRORING `core/burrow-ops/tests/conformance.rs`. The corpus fixes
   * rotate at "every page, by 90", and neither side can name every page without first
   * knowing how many there are — so both read the count, rotate `1..=count`, and read the
   * rotations back out of the EMITTED bytes rather than from what the operation meant to do.
   *
   * The composition is the TEST's, not the binding's. `burrow-wasm` exposes `rotate` and
   * `page_rotations`; deciding to call them in this order with this selection is a harness
   * decision, and it is made here rather than as a convenience entry point in the shipped
   * binding, which would be shipping a feature to serve a test.
   *
   * @param {string} base64
   * @param {{ password?: string | null, limits?: Record<string, number> }} [options]
   */
  async rotateEveryPage(base64, options = {}) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      bytes[i] = binary.charCodeAt(i);
    }
    const blob = new Blob([bytes], { type: "application/pdf" });

    const counted = await runOnBlob("page_rotations", blob, options);
    if (!counted.ok) return serialisable(counted);

    const pages = Array.from({ length: counted.pages }, (_, i) => i + 1);
    const turned = await runOnBlob("rotate", blob, { ...options, pages, degrees: 90 });
    if (!turned.ok || !turned.output) return serialisable(turned);

    // READ BACK OUT OF THE OUTPUT. A rotation cannot change the page count, so the rotations
    // are the only thing that tells a real rotation from a no-op.
    return serialisable(await runOnBlob("page_rotations", turned.output, options));
  },

  /**
   * Reverse the document's page order and report the rotations of the result.
   *
   * THREE OPERATIONS, MIRRORING `core/burrow-ops/tests/conformance.rs`, exactly as
   * `rotateEveryPage` does. The corpus fixes reorder at "reverse every page", and neither
   * side can name a permutation without first knowing how many pages there are — so both read
   * the count, reverse `1..=count`, and read the rotations back out of the EMITTED bytes.
   *
   * The rotations are the observable rather than something reorder-shaped, and that is not a
   * workaround: a page count cannot see a permutation, neither side has a per-page readout
   * except this one, and on a fixture whose pages differ a reversal must come back reversed.
   * `Operation::Reorder` carries the argument.
   *
   * The composition is the TEST's, not the binding's — `burrow-wasm` exposes `reorder` and
   * `page_rotations`, and deciding to call them in this order is a harness decision.
   *
   * @param {string} base64
   * @param {{ password?: string | null, limits?: Record<string, number> }} [options]
   */
  async reverseEveryPage(base64, options = {}) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      bytes[i] = binary.charCodeAt(i);
    }
    const blob = new Blob([bytes], { type: "application/pdf" });

    const counted = await runOnBlob("page_rotations", blob, options);
    if (!counted.ok) return serialisable(counted);

    const order = Array.from({ length: counted.pages }, (_, i) => counted.pages - i);
    const reordered = await runOnBlob("reorder", blob, { ...options, order });
    if (!reordered.ok || !reordered.output) return serialisable(reordered);

    return serialisable(await runOnBlob("page_rotations", reordered.output, options));
  },

  /**
   * Split a document after the given one-based pages and report how many parts came out.
   *
   * **The count, not the parts.** `Operation::Split` records a part count, and the conformance
   * corpus compares typed outcomes — so the observable that crosses back into a Playwright test
   * is a number, exactly as `merge`'s is a page count.
   *
   * The case this exists for records a REFUSAL rather than a count: the optional-content check
   * lives inside the shared pruning policy, so a path that skipped pruning would split happily
   * and report parts where the corpus expects `Unsupported`. That is the one divergence a single
   * shared policy can still have, and it is why this helper reports the failure faithfully
   * instead of turning it into zero parts.
   *
   * @param {string} base64
   * @param {{ password?: string | null, limits?: Record<string, number>, cuts?: number[] }} options
   */
  async splitAt(base64, options = {}) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      bytes[i] = binary.charCodeAt(i);
    }
    const blob = new Blob([bytes], { type: "application/pdf" });

    const reply = await runOnBlob("split", blob, { ...options, cuts: options.cuts ?? [1] });
    const flat = serialisable(reply);
    if (!flat.ok) return flat;
    // THE PART COUNT AS `pages`, so `outcomeOf` records it the way it records every other
    // operation's number. `serialisable` has already turned the Blobs into a count -- nothing
    // that crosses back into a test holds one.
    return { ...flat, pages: flat.partCount };
  },

  /** How many workers have been spawned. The recovery tests read this. */
  spawnCount: () => host.spawnCount(),

  /** Whether a live worker is currently held. */
  hasWorker: () => host.hasWorker(),

  /** The lifecycle state, by name. */
  state: () => host.state(),

  /** The recycling floor, as Rust reports it. See `worker-host.js`. */
  minConvergingMemoryBytes: () => host.minConvergingMemoryBytes(),

  /** `Limits::DEFAULT` as Rust reports it. The conformance harness builds its limits on it. */
  coreDefaultLimits: () => host.coreDefaultLimits(),

  /** Whether the circuit breaker has tripped. */
  breakerOpen: () => host.breakerOpen(),

  /** Close the breaker, as a "try again" button would. */
  reset: () => host.reset(),

  /**
   * Arm the test affordances for the NEXT worker.
   *
   * Takes effect on the next spawn, so a test that wants a poisoned worker discards the
   * current one first (or arms before `ready()`).
   */
  arm(options = {}) {
    prologue = buildPrologue(options);
  },

  /** Everything the worker wrote to its console since the page loaded. */
  workerConsole: () => workerConsole.slice(),

  /** Discard the current worker, so the next request spawns one with the armed prologue. */
  discardWorker() {
    host.discardWorker();
  },

  /**
   * Try to fetch `url` from INSIDE a worker, and report whether the browser refused.
   *
   * The page-level equivalent proves the policy applies to the document. This proves it
   * applies where the file bytes actually are — the page never sees them after the hand-off,
   * so a policy covering only the document would be the wrong way round and would look
   * identical from outside.
   */
  async fetchFromWorker(url) {
    // A probe worker built the same way the real one is: from a Blob, so it inherits this
    // page's policy. Constructing it from a URL instead would give it no policy, and the test
    // would report "not blocked" for the wrong reason — precisely the bug this API detects.
    //
    // `securitypolicyviolation` is reported alongside the outcome because "the fetch failed"
    // and "the browser refused the fetch" are different facts. A network error reaching an
    // unreachable port looks identical from the catch block; only the event says the policy is
    // what stopped it.
    const source = `
      self.addEventListener("securitypolicyviolation", (e) => {
        self.__violations.push(e.violatedDirective || e.effectiveDirective || "unknown");
      });
      self.__violations = [];
      self.onmessage = async (e) => {
        let blocked;
        try {
          await fetch(e.data.url, { mode: "no-cors" });
          blocked = false;
        } catch {
          blocked = true;
        }
        // The violation event is dispatched asynchronously; give it a turn to arrive.
        await new Promise((r) => setTimeout(r, 50));
        self.postMessage({ blocked, violations: self.__violations.slice() });
      };`;
    const probeUrl = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
    try {
      return await new Promise((resolve) => {
        const probe = new Worker(probeUrl);
        /** @param {import("./harness-api.js").ProbeResult} value */
        const done = (value) => {
          probe.terminate();
          resolve(value);
        };
        probe.onmessage = (event) => done(event.data);
        // A worker that cannot start has not made the request either, but that is a different
        // fact from "the fetch was blocked". Report it as not-blocked so a test fails loudly
        // rather than passing for the wrong reason.
        probe.onerror = () => done({ blocked: false, violations: [], failed: true });
        probe.postMessage({ url });
      });
    } finally {
      URL.revokeObjectURL(probeUrl);
    }
  },

  /**
   * Whether a FRESH worker accepts work — which it does only if a policy is in force.
   *
   * **Discards first, deliberately.** Without that this is a tautology: `host.ready()`
   * resolves `true` immediately when a worker is already `idle`, so after `openHarness()` it
   * would answer without any worker's fail-closed guard ever running. It was exactly that for
   * a while, and it silently disabled both callers —
   * `e2e/csp.spec.ts`'s "the worker refuses to touch a file unless it inherits the policy" and
   * `e2e/worker-guard.spec.ts`'s "the real worker is built from a blob and does the work".
   *
   * A spawn is ~80 ms (ADR 0015 §6), so making it real costs nothing worth saving.
   */
  async workerInheritsCsp() {
    host.discardWorker();
    return harness.ready();
  },
};

window.burrowHarness = harness;

// A visible signal for a human opening the page by hand, and what the tests wait on.
harness
  .ready()
  .then((ok) => {
    status.textContent = ok ? "engines ready" : "engines failed to initialise";
    result.textContent = JSON.stringify(ENGINES, null, 2);
  })
  .catch(() => {
    status.textContent = "engines failed to initialise";
  });
