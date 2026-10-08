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

/**
 * The worker bundles, as `createToolHost` sees them: where each one's source is, and how many
 * `starting` messages its start-up may use.
 *
 * READ OFF THE GENERATED DESCRIPTORS, not written down here. `modules` is the length of the
 * array `tools/stage-web-engines.mjs` generates into that bundle, so the count the host will
 * honour and the count the bundle actually sends come from one place. Both are 2 today; see
 * `EXPECTED_ENGINE_MODULES` for why that coincidence is the reason it is passed at all.
 */
const BUNDLES = /** @type {Record<string, { worker: EngineEntry, modules: number }>} */ (
  JSON.parse(required("engines").dataset.bundles ?? "{}")
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
 * The render bundle's host, and redaction's, each built on first use and never before.
 *
 * NOT BUILT AT LOAD TIME, and that is the property under test as much as any assertion: a
 * harness that spawned the render host eagerly would be measuring a page that downloads PDFium
 * on arrival, which is exactly what ADR 0026 exists to prevent. The same holds for redaction's
 * bundle (#137): nothing fetches it until something asks.
 *
 * MEMOISED AS A PROMISE, NOT A RESULT, which is the same fix `tool-host.ts` carries and for
 * the same reason: a guard read after an `await` lets two concurrent callers each build a host,
 * each spawn a worker with its own engine instance, and share one object URL so one revokes the
 * other's — leaving an orphan worker holding file bytes for the life of the page. Harness-only
 * here, and written the right way anyway: a rig that models the production wiring wrongly is a
 * rig whose green says nothing. Raised by security review.
 *
 * ONE BUILDER, KEYED BY BUNDLE, since the third bundle arrived: two copies of this were the
 * two-places-to-go-wrong shape.
 *
 * @type {Record<string, Promise<ReturnType<typeof createWorkerHost>>>}
 */
const lazyHosts = {};
/** @type {Record<string, string | null>} */
const lazyUrls = {};

/** @param {"renderWorker" | "redactWorker"} id */
function bundleHost(id) {
  lazyHosts[id] ??= buildBundleHost(id);
  return lazyHosts[id];
}

function renderBundle() {
  return bundleHost("renderWorker");
}

/** @param {"renderWorker" | "redactWorker"} id */
async function fetchBundleSource(id) {
  const bundle = BUNDLES[id];
  if (!bundle) {
    // Redaction's bundle is staged into harness builds only (#137); a harness page served from
    // anything else has no entry for it, and says so rather than failing inside `fetch`.
    throw new Error(`no ${id} bundle in this build`);
  }
  const response = await fetch(bundle.worker.url, { integrity: bundle.worker.integrity });
  if (!response.ok) {
    throw new Error(`${id} fetch failed`);
  }
  return { bundle, source: await response.text() };
}

// --- redaction's worker, as R8 and R9 observe it (#137) ------------------------------------------
//
// ADR 0006's R8 and R9 are claims about WHAT REDACTION'S WORKER SENDS AND DOES, so their specs
// need four things no other test did: every message between the page and the worker, recorded
// before the host reads it; stubs in the worker's scope for the ways bytes could leave the heap;
// a way to wait until the worker has gone quiet, so "posted once" is not "posted once so far";
// and a COPY of the worker with a planted violation, to show each spec can fail. All of them act
// on the integrity-checked text, like the base worker's prologue does.

/** @type {{ prologue: string, source: string | null }} */
let redactArming = { prologue: "", source: null };
/**
 * Every message redaction's worker posted since the last arming, described, in order.
 *
 * @type {import("./harness-api").RedactionMessage[]}
 */
let redactLog = [];
/**
 * The raw redaction worker, for `settleRedaction` -- which must post around the host, because
 * the host has no message for "tell me when you are quiet".
 *
 * @type {{ worker: Worker, send: (message: unknown) => void } | null}
 */
let redactRaw = null;
/**
 * The host's `terminate()` for a redaction worker it has discarded, HELD until the settle
 * handshake has run (#199). Every redaction now recycles its worker, so the host terminates it
 * straight after the reply -- and R8's "posted once, nothing after" needs the worker alive for the
 * settle window, or the handshake never answers. Held, not skipped: it runs when the settle
 * completes, or when the next redaction worker starts, so at most one deferred worker is alive.
 *
 * @type {(() => void) | null}
 */
let redactPendingTerminate = null;
/** How many times the host asked to terminate a redaction worker since the last arming. */
let redactTerminations = 0;
/**
 * What the heap canary saw (#199): one entry per reply redaction's worker posted while armed with
 * `heapCanary`, each listing every WebAssembly memory the worker had instantiated.
 *
 * @type {import("./harness-api").HeapScan[]}
 */
let redactHeapScans = [];

/**
 * How many byte-carrying values a message holds -- each `ArrayBuffer`, typed array or `Blob`,
 * at any depth. NOT A RULE: R8's rules are the shapes below, which refuse bytes anywhere but the
 * reply's `output`. This is the recorder's own witness, so a spec can require that it saw the
 * document at all.
 *
 * @param {unknown} value
 * @param {Set<object>} seen
 * @returns {number}
 */
function countBytes(value, seen = new Set()) {
  if (value instanceof ArrayBuffer || ArrayBuffer.isView(value) || value instanceof Blob) {
    return 1;
  }
  if (value === null || typeof value !== "object" || seen.has(value)) return 0;
  seen.add(value);
  return Object.values(value).reduce((sum, item) => sum + countBytes(item, seen), 0);
}

/**
 * A message's TYPE, as a canonical string, DENY-BY-DEFAULT: a plain object is its sorted keys and
 * each value's type, an array the set of its elements' types, `true`/`false` themselves, a byte
 * value `bytes`, and anything else -- an Error, a boxed String, a ReadableStream, a Map, an
 * ImageData, an array carrying named properties -- is `other:<name>`, which no listed shape
 * contains. Each of those carried a whole document past a
 * walker that listed what to look at rather than what to allow (review of #137).
 *
 * @param {unknown} value
 * @param {Set<object>} seen
 * @returns {string}
 */
function shapeOf(value, seen = new Set()) {
  if (value === null) return "null";
  if (typeof value === "boolean") return String(value);
  if (typeof value !== "object") return typeof value;
  if (seen.has(value)) return "cycle";
  seen.add(value);
  const prototype = Object.getPrototypeOf(value);
  // A PLAIN BLOB, not a File: a File's name carried the whole document past `instanceof Blob`
  // (review of #137). A Blob's `type` is text too, and `fieldsOf` records it for a rule.
  if (value instanceof ArrayBuffer || ArrayBuffer.isView(value) || prototype === Blob.prototype) {
    return "bytes";
  }
  // AN ARRAY IS ITS ELEMENTS AND NOTHING ELSE: structured clone carries an array's named
  // properties too, and a document went past the first version as 426 of them (review of #137).
  if (
    Array.isArray(value) &&
    prototype === Array.prototype &&
    Reflect.ownKeys(value).length === value.length + 1
  ) {
    return `[${[...new Set(value.map((item) => shapeOf(item, seen)))].sort().join("|")}]`;
  }
  if (prototype === Object.prototype || prototype === null) {
    const fields = Reflect.ownKeys(value).map((key) =>
      typeof key === "string"
        ? `${key}:${shapeOf(/** @type {any} */ (value)[key], seen)}`
        : "symbol",
    );
    return `{${fields.sort().join(",")}}`;
  }
  return `other:${prototype?.constructor?.name ?? "?"}`;
}

/** @param {unknown} value */
function isPort(value) {
  return typeof MessagePort !== "undefined" && value instanceof MessagePort;
}

/**
 * One message, described without its content.
 *
 * THE ID IS A NUMBER OR NULL, and nothing else: a message whose `id` is `undefined` or a string
 * names no request, and recording it verbatim let one be neither (review of #137).
 *
 * @param {any} data
 * @param {number} ports
 */
function describe(data, ports) {
  const object = data !== null && typeof data === "object";
  return {
    id: object && typeof data.id === "number" ? data.id : null,
    keys: object ? Object.keys(data).sort() : [],
    bytes: countBytes(data),
    shape: shapeOf(data),
    fields: object ? fieldsOf(data) : {},
    ports,
    ok: object && typeof data.ok === "boolean" ? data.ok : null,
    sideChannel:
      object && typeof data.__burrowSideChannel === "string" ? data.__burrowSideChannel : null,
    armed:
      object && Array.isArray(data.__burrowSideChannelArmed) ? data.__burrowSideChannelArmed : null,
    settled: object && typeof data.__burrowSettled === "number" ? data.__burrowSettled : null,
  };
}

/**
 * Every primitive value in a message, one level of plain objects deep (`defaultLimits.maxPages`),
 * and a Blob's `type` -- EXCEPT a refusal's `message`: Rust's prose, which R8 states it does not
 * check. What
 * R8 holds each field's VALUE to, where the shape holds only its type -- a shape admits any
 * string, and 60 bytes of a document went past the version that checked only length, in `stage`
 * (review of #137).
 *
 * @param {Record<string, unknown>} data
 * @returns {Record<string, string | number | boolean | null>}
 */
function fieldsOf(data) {
  /** @type {Record<string, string | number | boolean | null>} */
  const fields = {};
  for (const [key, value] of Object.entries(data)) {
    // A REFUSAL'S `message` is Rust's prose, and the one thing R8 states it does not check. A
    // success has none, so there it is recorded -- and must be empty.
    if (key === "message" && data.ok !== true) continue;
    // A REFUSAL'S `report` is empty, and is recorded under its own name so a rule can say so: the
    // report's grammar admitted six hundred bytes of numbers on a refusal (review of #137).
    if (key === "report" && data.ok === false) {
      fields.refusalReport = /** @type {string} */ (value);
      continue;
    }
    if (value instanceof Blob) fields[`${key}.type`] = value.type;
    if (value === null || ["string", "number", "boolean"].includes(typeof value)) {
      fields[key] = /** @type {string | number | boolean | null} */ (value);
    } else if (
      value !== null &&
      typeof value === "object" &&
      Object.getPrototypeOf(value) === Object.prototype
    ) {
      for (const [inner, innerValue] of Object.entries(value)) {
        if (innerValue === null || ["string", "number", "boolean"].includes(typeof innerValue)) {
          fields[`${key}.${inner}`] = /** @type {string | number | boolean | null} */ (innerValue);
        }
      }
    }
  }
  return fields;
}

/**
 * Record every message a redaction worker posts, BEFORE the host sees it, and keep the harness's
 * own tagged messages away from the host.
 *
 * @param {Worker} worker
 * @returns {Worker}
 */
function recordRedaction(worker) {
  /** @type {{ current: ((event: { data: any }) => void) | null }} */
  const hostHandler = { current: null };
  Object.defineProperty(worker, "onmessage", {
    configurable: true,
    get: () => hostHandler.current,
    set: (handler) => {
      hostHandler.current = handler;
    },
  });
  // WHAT THE PAGE SENT, TOO, so a reply's id is attributed to the request that carried it rather
  // than guessed: the host's start-up handshake takes an id from the same counter as an operation.
  const send = worker.postMessage.bind(worker);
  worker.postMessage = (/** @type {any} */ message, /** @type {any} */ transfer) => {
    redactLog.push({
      sent: String(message?.op ?? message?.type ?? "?"),
      ...describe(message, Array.isArray(transfer) ? transfer.filter(isPort).length : 0),
    });
    return transfer === undefined ? send(message) : send(message, transfer);
  };
  redactRaw = { worker, send };
  // A PREVIOUS WORKER STILL HELD is let go now: a new one is starting, so nothing waits on it.
  redactPendingTerminate?.();
  redactPendingTerminate = null;
  const terminate = worker.terminate.bind(worker);
  worker.terminate = () => {
    redactTerminations++;
    redactPendingTerminate = () => terminate();
  };
  worker.addEventListener("message", (/** @type {MessageEvent} */ event) => {
    const data = event.data;
    redactLog.push({ sent: null, ...describe(data, event.ports.length) });
    if (data?.__burrowHeapScan) {
      redactHeapScans.push(data.__burrowHeapScan);
      return;
    }
    if (data?.__burrowSideChannel || data?.__burrowSideChannelArmed || data?.__burrowSettled) {
      return;
    }
    hostHandler.current?.(event);
  });
  return worker;
}

/**
 * The settle handshake, installed whenever redaction's worker is armed: a NONCE the page sends
 * around the host, echoed after `ms` on the worker's clock.
 *
 * WHAT THE WORKER CANNOT DO TO IT, and why each holds:
 * - Forge it: it never sees the nonce -- this listener is registered first, in the capture
 *   phase, and stops the handshake -- and a marker with any other value is a violation.
 * - Shorten it: WHAT BOUNDS THE WINDOW IS THE PAGE'S CLOCK, not this one. The page waits until
 *   `2 * ms` after it sent the handshake whenever the echo comes back; the echo proves only that
 *   the worker is alive and took the handshake. (`setTimeout` and `postMessage` are captured here
 *   anyway, so a bundle that replaces them does not also silence the echo.)
 * - Suppress it: a worker that never answers fails closed -- `settled` is false.
 */
const SETTLE = `
(() => {
  const post = self.postMessage.bind(self);
  const later = self.setTimeout.bind(self);
  self.addEventListener(
    "message",
    (event) => {
      const nonce = event.data && event.data.__burrowSettle;
      if (typeof nonce !== "number") return;
      event.stopImmediatePropagation();
      later(() => post({ __burrowSettled: nonce }), event.data.ms);
    },
    // CAPTURE, and first: Firefox and WebKit run a capture listener registered LATER before a
    // non-capture one registered first, so a bundle could read the nonce and echo it early
    // (review of #137). Registered first in the capture phase, nothing precedes this one.
    { capture: true },
  );
})();
`;

/**
 * The heap canary (#199): count `canary`'s occurrences in every WebAssembly memory redaction's
 * worker holds, at the moment it posts a reply -- after the Rust call has returned and the
 * worker's `finally` has wiped its own copies, and before the page can recycle the worker.
 *
 * HARNESS-ONLY BY CONSTRUCTION, like every prologue here: it is text this driver prepends to the
 * integrity-checked bundle, and the production host never builds a prologue at all.
 *
 * WHICH MEMORY IS WHICH. Every memory the worker instantiates is recorded -- exported or imported,
 * through `WebAssembly.instantiate`, `instantiateStreaming` or `new WebAssembly.Instance` -- and the
 * one behind the qpdf module the bridge holds (`__burrow_attach`) is labelled `qpdf`; every other
 * is `burrow`. A scan reports how many it examined, so a capture that missed one says so rather
 * than reading as clean.
 *
 * @param {string} canary
 */
function heapScanPrologue(canary) {
  return `
(() => {
  const needle = new TextEncoder().encode(${JSON.stringify(canary)});
  const memories = [];
  const keep = (value) => {
    if (value instanceof WebAssembly.Memory && !memories.includes(value)) memories.push(value);
  };
  const fromImports = (imports) => {
    for (const space of Object.values(imports || {})) {
      for (const value of Object.values(space || {})) keep(value);
    }
  };
  const fromExports = (exports) => {
    for (const value of Object.values(exports || {})) keep(value);
  };
  const instantiate = WebAssembly.instantiate.bind(WebAssembly);
  WebAssembly.instantiate = async (source, imports) => {
    fromImports(imports);
    const made = await instantiate(source, imports);
    fromExports((made.instance || made).exports);
    return made;
  };
  if (WebAssembly.instantiateStreaming) {
    const streaming = WebAssembly.instantiateStreaming.bind(WebAssembly);
    WebAssembly.instantiateStreaming = async (source, imports) => {
      fromImports(imports);
      const made = await streaming(source, imports);
      fromExports(made.instance.exports);
      return made;
    };
  }
  const Instance = WebAssembly.Instance;
  WebAssembly.Instance = function (module, imports) {
    fromImports(imports);
    const made = new Instance(module, imports);
    fromExports(made.exports);
    return made;
  };
  WebAssembly.Instance.prototype = Instance.prototype;
  let qpdf = null;
  // THE PEAK WHILE THE OPERATION RUNS, per memory: the witness. Sampled on entry to every
  // STRIDE-th bridge call, all the way through -- a redaction makes about 1,400, and a Flate
  // page's decoded text first appears around the 445th, so the first version's cap of 64 calls
  // saw none of it and the compressed case was dropped for a property of the scanner (#199's
  // review). A heap whose peak is zero is one this scan cannot vouch for.
  const STRIDE = 8;
  let calls = 0;
  const peak = new Map();
  const sample = () => {
    if (calls++ % STRIDE !== 0) return;
    for (const memory of memories) {
      const hits = count(new Uint8Array(memory.buffer));
      peak.set(memory, Math.max(peak.get(memory) || 0, hits));
    }
  };
  self.addEventListener("message", () => {
    const attach = self.__burrow_attach;
    if (typeof attach !== "function" || attach.__heapScan) return;
    const wrapped = (module) => {
      qpdf = module;
      return attach(module);
    };
    wrapped.__heapScan = true;
    self.__burrow_attach = wrapped;
    for (const name of Object.keys(self)) {
      const original = self[name];
      if (!name.startsWith("__burrow_qpdf_") || typeof original !== "function") continue;
      self[name] = function (...args) {
        sample();
        return original.apply(this, args);
      };
    }
  }, { once: true, capture: true });
  const count = (heap) => {
    let hits = 0;
    const first = needle[0];
    for (let at = heap.indexOf(first); at !== -1 && at <= heap.length - needle.length; at = heap.indexOf(first, at + 1)) {
      let match = true;
      for (let i = 1; i < needle.length; i++) {
        if (heap[at + i] !== needle[i]) { match = false; break; }
      }
      if (match) hits++;
    }
    return hits;
  };
  const post = self.postMessage.bind(self);
  self.postMessage = (message, transfer) => {
    if (message && typeof message === "object" && typeof message.ok === "boolean" && "kind" in message) {
      const qpdfBuffer = qpdf && qpdf.HEAPU8 ? qpdf.HEAPU8.buffer : null;
      post({
        __burrowHeapScan: memories.map((memory) => ({
          heap: memory.buffer === qpdfBuffer ? "qpdf" : "burrow",
          bytes: memory.buffer.byteLength,
          hits: count(new Uint8Array(memory.buffer)),
          peak: peak.get(memory) || 0,
        })),
      });
    }
    return transfer === undefined ? post(message) : post(message, transfer);
  };
})();
`;
}

/**
 * Stubs for R9's exits, installed in the worker's scope before the bundle runs.
 *
 * Each still does what it did -- a stub that broke the call would change what the worker does
 * next -- and reports that it was called. A stub installs ONLY where the thing it wraps exists:
 * defining `showSaveFilePicker` in a scope that has none would change what feature detection in
 * the worker sees, and report as "watched" an exit nobody can take (review of #137). The list of
 * stubs that installed is posted first, so a spec can tell "never called" from "never watched".
 *
 * THE LIST IS ENUMERATED BY HAND, and is not every way out there is. It is ADR 0006's three, and
 * every other one found so far by trying to get bytes to the page past it: a broadcast, two
 * stores, a lock's name, and a nested worker, whose own stubs would report to its parent rather
 * than to this page (all three found by the review of #137).
 */
const SIDE_CHANNEL_STUBS = `
(() => {
  const armed = [];
  // CAPTURED, so a bundle that filters \`self.postMessage\` cannot silence a report (review of #137).
  const post = self.postMessage.bind(self);
  const tell = (name) => post({ __burrowSideChannel: name });
  const wrap = (owner, key, name) => {
    const original = owner && owner[key];
    if (typeof original !== "function") return;
    try {
      Object.defineProperty(owner, key, {
        configurable: true,
        writable: true,
        value: function (...args) {
          tell(name);
          return original.apply(this, args);
        },
      });
      armed.push(name);
    } catch {}
  };
  const wrapConstructor = (key) => {
    const original = self[key];
    if (typeof original !== "function") return;
    const watched = new Proxy(original, {
      construct(target, args, newTarget) {
        tell(key);
        return Reflect.construct(target, args, newTarget);
      },
    });
    self[key] = watched;
    // AND THE PROTOTYPE'S BACK-REFERENCE: the Proxy forwards \`.prototype\` to its target, so
    // \`Worker.prototype.constructor\` was the unwatched original (review of #137).
    try {
      Object.defineProperty(original.prototype, "constructor", {
        configurable: true,
        writable: true,
        value: watched,
      });
    } catch {
      return;
    }
    armed.push(key);
  };
  wrap(URL, "createObjectURL", "createObjectURL");
  wrap(self, "showSaveFilePicker", "showSaveFilePicker");
  if (typeof StorageManager !== "undefined") wrap(StorageManager.prototype, "getDirectory", "getDirectory");
  if (typeof BroadcastChannel !== "undefined") wrap(BroadcastChannel.prototype, "postMessage", "BroadcastChannel");
  if (typeof IDBFactory !== "undefined") wrap(IDBFactory.prototype, "open", "indexedDB");
  if (typeof CacheStorage !== "undefined") wrap(CacheStorage.prototype, "open", "caches");
  if (typeof LockManager !== "undefined") wrap(LockManager.prototype, "request", "locks");
  wrapConstructor("Worker");
  wrapConstructor("SharedWorker");
  post({ __burrowSideChannelArmed: armed });
})();
`;

/** The armed list when no stub is armed: empty, and still first. */
const NO_STUBS = `
self.postMessage({ __burrowSideChannelArmed: [] });
`;

/** @param {"renderWorker" | "redactWorker"} id */
async function buildBundleHost(id) {
  const fetched = await fetchBundleSource(id);
  const bundle = fetched.bundle;
  const redaction = id === "redactWorker";
  const source = redaction && redactArming.source !== null ? redactArming.source : fetched.source;
  const prologue = redaction ? redactArming.prologue : "";
  return createWorkerHost({
    spawn: () => {
      const previous = lazyUrls[id];
      if (previous) URL.revokeObjectURL(previous);
      const url = URL.createObjectURL(new Blob([prologue, source], { type: "text/javascript" }));
      lazyUrls[id] = url;
      const worker = new Worker(url);
      return redaction ? recordRedaction(worker) : worker;
    },
    release: () => {
      const previous = lazyUrls[id];
      if (previous) {
        URL.revokeObjectURL(previous);
        lazyUrls[id] = null;
      }
    },
    now: () => performance.now(),
    setTimer: (fn, ms) => setTimeout(fn, ms),
    clearTimer: (handle) => clearTimeout(/** @type {number} */ (handle)),
    maxDurationMs: DEFAULT_LIMITS.maxDurationMs,
    // PER BUNDLE. See `EXPECTED_ENGINE_MODULES` for why the count is passed rather than assumed.
    expectedEngineModules: bundle.modules,
  });
}

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
   * Compress the document and report the page count, the rotations, and whether it shrank.
   *
   * TWO OPERATIONS, MIRRORING `core/burrow-ops/tests/conformance.rs`, as `rotateEveryPage`
   * does. Compression takes no selection, so there is no count to read first — but the
   * rotations still have to come out of the EMITTED bytes, which means a second call.
   *
   * **`compressed` is the third observable and the one this case exists for.** A page count
   * and a rotation vector are identical whether or not the object stream mode was set, so a
   * comparison of only those would pass against a path that had quietly become a plain write.
   * `Operation::Compress` carries the argument, including why it is a boolean rather than a
   * size.
   *
   * It is derived from the two counts the reply carries rather than from whether an output
   * arrived. The two are equivalent by the operation's contract — `Smaller` is the only
   * variant that returns bytes — and taking the counts exercises the numbers a tool page will
   * actually show, rather than a proxy for them.
   *
   * ON THE NOT-SMALLER BRANCH there are no emitted bytes, by design, so the rotations are read
   * from the INPUT. That is not a fallback: the operation returned nothing precisely because
   * it promised the caller's own document is already the better one, and what it promised not
   * to change is what gets checked.
   *
   * @param {string} base64
   * @param {{ password?: string | null, limits?: Record<string, number> }} [options]
   */
  async compressDocument(base64, options = {}) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) {
      bytes[i] = binary.charCodeAt(i);
    }
    const blob = new Blob([bytes], { type: "application/pdf" });

    const done = await runOnBlob("compress", blob, options);
    if (!done.ok) return serialisable(done);

    // BOTH COUNTS MUST BE THERE. They are optional on `HostReply` because only `compress`
    // computes them, so a successful compress without them is a broken reply -- and the
    // tempting `?? "0"` would turn that into `0 < 0`, reporting "did not compress" for every
    // document. That is the differential harness's own verdict silently inverted, which is
    // exactly the failure it exists to catch. Refuse instead.
    if (done.originalBytes === undefined || done.producedBytes === undefined) {
      return { ...serialisable(done), ok: false, kind: "Internal" };
    }

    // BigInt, because `drainReply` sends both counts as strings: they are `u64` in Rust and
    // rounding a size somebody reads would be a small lie with no upside.
    const compressed = BigInt(done.producedBytes) < BigInt(done.originalBytes);
    const source = done.output ?? blob;
    const read = await runOnBlob("page_rotations", source, options);
    return { ...serialisable(read), compressed };
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

  /**
   * Arm redaction's worker for the R8 and R9 specs (#137), and start the next redaction on a
   * fresh one: `stubSideChannels` installs R9's stubs before the bundle runs, and `mutate`
   * applies ONE replacement to a copy of the bundle's text -- the planted violation a spec must
   * catch. Returns whether the replacement applied, because a mutation that matched nothing
   * leaves the real worker in place and measures nothing. Clears the message log.
   *
   * `heapCanary` installs the heap canary (#199): every reply is preceded by a scan of every
   * WebAssembly memory in the worker for that text, read back with `redactHeapScans`.
   *
   * @param {{ stubSideChannels?: boolean, mutate?: { from: string, to: string } | null, heapCanary?: string }} options
   */
  async armRedaction(options = {}) {
    let source = null;
    let applied = false;
    if (options.mutate) {
      const { from, to } = options.mutate;
      source = (await fetchBundleSource("redactWorker")).source;
      const at = source.indexOf(from);
      if (at !== -1 && source.indexOf(from, at + 1) === -1) {
        // SLICED, NOT `replace`: a string replacement expands `$&` and its kin, so the planted
        // text could differ from the text asked for while this still said it applied.
        source = source.slice(0, at) + to + source.slice(at + from.length);
        // The unique match above is what decides it; a splice cannot then fail to apply.
        applied = true;
      }
    }
    redactArming = {
      // THE ARMED LIST IS ALWAYS POSTED, EMPTY WHEN NOTHING IS ARMED: R8 admits it only as the
      // worker's first message, and with no prologue list there a bundle's own post was first --
      // and a forged list then vouched for any number of forged reports (review of #137).
      prologue:
        SETTLE +
        (options.stubSideChannels ? SIDE_CHANNEL_STUBS : NO_STUBS) +
        (options.heapCanary ? heapScanPrologue(options.heapCanary) : ""),
      source,
    };
    redactHeapScans = [];
    const existing = lazyHosts.redactWorker;
    delete lazyHosts.redactWorker;
    if (existing) (await existing).dispose();
    redactLog = [];
    redactRaw = null;
    redactTerminations = 0;
    return { applied };
  },

  /**
   * Wait until redaction's worker has gone quiet: send a nonce around the host, wait for its echo,
   * and in any case until `2 * ms` after the send on the page's own clock. Anything the worker
   * posted in that window has been recorded; anything later has not, and the specs say so.
   * `settled` is false when no echo arrived within `ms` plus five seconds.
   *
   * @param {number} ms
   */
  async settleRedaction(ms = 500) {
    if (redactRaw === null) return { settled: false, nonce: 0 };
    const { worker, send } = redactRaw;
    const nonce = 1 + Math.floor(Math.random() * 2 ** 31);
    const deadline = performance.now() + 2 * ms;
    const echoed = await new Promise((resolve) => {
      const timer = setTimeout(() => {
        worker.removeEventListener("message", heard);
        resolve(false);
      }, ms + 5_000);
      /** @param {MessageEvent} event */
      function heard(event) {
        if (event.data?.__burrowSettled !== nonce) return;
        clearTimeout(timer);
        worker.removeEventListener("message", heard);
        resolve(true);
      }
      worker.addEventListener("message", heard);
      send({ __burrowSettle: nonce, ms });
    });
    // THE PAGE'S CLOCK DECIDES, measured from the send: an echo that came back early -- however
    // it did -- does not shorten the window.
    if (echoed) {
      await new Promise((resolve) =>
        setTimeout(resolve, Math.max(0, deadline - performance.now())),
      );
    }
    // THE SETTLE IS DONE, so a termination the host asked for in the meantime happens now.
    redactPendingTerminate?.();
    redactPendingTerminate = null;
    return { settled: Boolean(echoed), nonce };
  },

  /**
   * How many times the host asked to terminate a redaction worker since the last arming -- a
   * recycle or a discard. The termination itself is held until a settle (see
   * `redactPendingTerminate`).
   */
  redactTerminations() {
    return redactTerminations;
  },

  /** What the heap canary saw since the last arming: one scan per reply (#199). */
  redactHeapScans() {
    return redactHeapScans.map((scan) => scan.map((entry) => ({ ...entry })));
  },

  /** Every message between the page and redaction's worker since the last arming, in order. */
  redactMessages() {
    return redactLog.map((entry) => ({ ...entry }));
  },

  /**
   * Redact one region on one page through redaction's own worker (#137), and report what came
   * back: the reply's fields, and the output's sha256 -- never the bytes, which stay in the
   * page. `tests/redaction/outcomes.tsv` pins digests, so a digest is what a test compares.
   *
   * `page` and `covered` are 1-based, as the worker protocol is.
   *
   * @param {string} name
   * @param {Uint8Array} bytes
   * @param {number} page
   * @param {number[]} covered
   * @param {{ left: number, top: number, width: number, height: number }} region
   * @param {{ limits?: Record<string, number> }} options
   */
  async redactDocument(name, bytes, page, covered, region, options = {}) {
    const worker = await bundleHost("redactWorker");
    const reply = await worker.run(
      {
        op: "redact",
        blob: new File([/** @type {Uint8Array<ArrayBuffer>} */ (bytes)], name, {
          type: "application/pdf",
        }),
        page,
        covered,
        region,
        limits: { ...DEFAULT_LIMITS, ...(options.limits ?? {}) },
      },
      { maxDurationMs: DEFAULT_LIMITS.maxDurationMs },
    );
    const { output, ...fields } = reply;
    let outputSha256 = null;
    if (output instanceof Blob) {
      const digest = await crypto.subtle.digest("SHA-256", await output.arrayBuffer());
      outputSha256 = Array.from(new Uint8Array(digest), (b) =>
        b.toString(16).padStart(2, "0"),
      ).join("");
    }
    return { ...fields, outputSha256 };
  },

  // --- the render bundle ----------------------------------------------------------------
  //
  // ADR 0026 ships a SECOND worker bundle: PDFium, fetched only when a page needs a picture
  // of a page. It is driven here so that a browser test can exercise it end to end -- through
  // the real integrity-pinned fetch, the real `blob:` construction, the real fail-closed CSP
  // guard and the real lifecycle -- rather than the split being asserted only over files on
  // disk. An untested worker whose guard decides whether file bytes may be touched is the
  // thing that goes wrong quietly.
  //
  // ONE LIFECYCLE, TWO INSTANCES, which is the claim this exercises as much as the loading.
  // The render host is `createWorkerHost` again, with the bundle and its module count as
  // arguments; there is no second state machine, no second watchdog and no second breaker
  // implementation. The breaker STATE is independent because the instances are, and that is
  // deliberate: a document that kills the renderer must not take merging offline.
  async renderPageCount(name, bytes, options = {}) {
    const worker = await renderBundle();
    const reply = await worker.run(
      {
        op: "page_count",
        blob: new File([new Uint8Array(bytes)], name, { type: "application/pdf" }),
        limits: { ...DEFAULT_LIMITS, ...(options.limits ?? {}) },
      },
      { maxDurationMs: DEFAULT_LIMITS.maxDurationMs },
    );
    return reply;
  },

  /**
   * Draw a strip, and report how long each page took.
   *
   * **For `measure.spec.ts`, which is where the render watchdog budget comes from.** The
   * document operations' budget was derived from timing the slowest honest operation in the
   * corpus (`LIMITS.maxDurationMs`, 12 s from a 611 ms worst case); a thumbnail is a different
   * workload and inherits that number only because nothing had measured it.
   *
   * Timed PER PAGE rather than for the strip, because the budget is per page: the host re-arms
   * its watchdog on every tile.
   *
   * @param {string} name
   * @param {number[]} bytes
   * @param {number[]} pages
   * @param {{ boxWidth?: number, boxHeight?: number, limits?: Record<string, number> }} options
   */
  async renderStrip(name, bytes, pages, options = {}) {
    const worker = await renderBundle();
    /** @type {number[]} */
    const perPage = [];
    let last = performance.now();
    const reply = await worker.run(
      {
        op: "render",
        blob: new File([new Uint8Array(bytes)], name, { type: "application/pdf" }),
        pages,
        boxWidth: options.boxWidth ?? 240,
        boxHeight: options.boxHeight ?? 320,
        limits: { ...DEFAULT_LIMITS, ...(options.limits ?? {}) },
      },
      {
        maxDurationMs: DEFAULT_LIMITS.maxDurationMs,
        onPage: () => {
          const now = performance.now();
          perPage.push(now - last);
          last = now;
        },
      },
    );
    return { ok: reply.ok, kind: reply.kind, drawn: perPage.length, perPage };
  },

  /**
   * Draw one page and hand back its RAW PIXELS, for the differential corpus.
   *
   * **The grid is computed on the other side**, by `src/conformance/ink-grid.ts`, which is
   * tested against the value the corpus records. Computing it here would put a second copy of
   * the quantiser in a file no unit test can reach — and a differential corpus whose two sides
   * quantise differently is comparing its own quantisers rather than the engines.
   *
   * @param {string} name
   * @param {number[]} bytes
   * @param {{ password?: string | null, limits?: Record<string, number>, boxWidth?: number,
   *   boxHeight?: number }} options
   */
  async renderForCorpus(name, bytes, options = {}) {
    const worker = await renderBundle();
    /** @type {{ width: number, height: number, rgba: number[] } | null} */
    let drawn = null;
    const reply = await worker.run(
      {
        op: "render",
        blob: new File([new Uint8Array(bytes)], name, { type: "application/pdf" }),
        pages: [1],
        boxWidth: options.boxWidth ?? 64,
        boxHeight: options.boxHeight ?? 128,
        password: options.password ? Array.from(new TextEncoder().encode(options.password)) : null,
        limits: { ...DEFAULT_LIMITS, ...(options.limits ?? {}) },
      },
      {
        maxDurationMs: DEFAULT_LIMITS.maxDurationMs,
        onPage: (page, pixels) => {
          drawn = {
            width: page.width,
            height: page.height,
            rgba: Array.from(new Uint8Array(pixels)),
          };
        },
      },
    );
    return {
      ok: reply.ok && drawn !== null,
      kind: reply.kind,
      fatal: reply.fatal,
      message: reply.message,
      limit: reply.limit,
      stage: reply.stage,
      requested: reply.requested,
      allowed: reply.allowed,
      drawn,
    };
  },

  renderState() {
    // "unbuilt" is the property under test: nothing may build this host until something asks
    // for a render. `built` rather than the host's own state once it exists, because the
    // promise may still be in flight and a state read is not worth awaiting for.
    return lazyHosts.renderWorker === undefined ? "unbuilt" : "built";
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
