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
    parts.push(`
      self.addEventListener("message", () => {
        const original = self.__burrow_pdfium_copy_in;
        self.__burrow_pdfium_copy_in = (bytes) => {
          const until = Date.now() + ${hangMs};
          while (Date.now() < until) {}
          return original(bytes);
        };
      }, { once: true });`);
  }
  return parts.length > 0 ? `(() => {${parts.join("\n")}})();\n` : "";
}

// --- the API the Playwright tests drive ------------------------------------------------

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

  /** How many workers have been spawned. The recovery tests read this. */
  spawnCount: () => host.spawnCount(),

  /** Whether a live worker is currently held. */
  hasWorker: () => host.hasWorker(),

  /** The lifecycle state, by name. */
  state: () => host.state(),

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
