// The main-thread worker lifecycle, as an explicit state machine.
//
// WHY THIS IS A STATE MACHINE AND NOT A FEW FLAGS
//
// ADR 0009's web contract is enforced *here*, by page code, and nowhere else. On
// `wasm32-unknown-unknown` a Rust panic is an uncatchable trap: the worker survives it, no
// `error` event fires, and calls afterwards may still appear to work. Nothing forces our
// hand. The page must treat the *result* as the signal and discard the instance itself.
//
// A contract with no type system behind it needs the next best thing: a small set of named
// states, one place where each transition happens, and a test for every edge — including the
// ones that only occur when two things race. M1 PR 4a-i implemented a rough version of this
// as forty lines of ad-hoc page code inside the test harness, with no state model and no unit
// tests. The edges it got wrong are the ones this file names.
//
// THE FIVE STATES
//
//   idle          a live, initialised worker, doing nothing
//   initialising  the FIRST worker is spawning and running engine init
//   busy          a live worker is running one operation
//   dead          no worker; the next request must respawn (or be refused)
//   respawning    a REPLACEMENT worker is spawning and running engine init
//
// `initialising` and `respawning` do the same work and are deliberately distinct anyway:
// "a request arrived while a respawn was in progress" is one of the awkward cases that must
// be observable, and collapsing them would make it unobservable.
//
// EVERYTHING IS INJECTED
//
// No `Worker`, no `performance`, no `setTimeout`, no DOM. That is ADR 0007's reason for an
// injectable clock applied to the page: a watchdog test that really waits thirty seconds is a
// test nobody runs, and a respawn test that really compiles several megabytes of WebAssembly is worse.
// `worker-host.test.ts` drives every state below in milliseconds against a fake worker.

/**
 * @typedef {object} WorkerLike
 * @property {(message: unknown) => void} postMessage No transfer list: the input is a Blob,
 *   which structured clone passes by reference, so there is nothing to transfer. See `run`.
 * @property {() => void} terminate
 * @property {((event: MessageEvent) => void) | null} onmessage
 * @property {((event: ErrorEvent) => void) | null} onerror
 */

/**
 * @typedef {"idle" | "initialising" | "busy" | "dead" | "respawning"} HostState
 */

/**
 * @typedef {object} HostReply
 * @property {boolean} ok
 * @property {string} kind
 * @property {boolean} fatal
 * @property {string} message
 * @property {number} pages
 * @property {string} limit
 * @property {string} stage
 * @property {string} requested
 * @property {string} allowed
 * @property {boolean} recycle
 * @property {string} qpdfHeapBytes
 * @property {number} [failedInput] Which input failed, or -1.
 * @property {string} [innerKind] What was wrong with that input, or empty.
 * @property {Blob[]} [parts] The documents a MULTI-OUTPUT operation produced, in order.
 *
 *   Present only on a successful split, and only when every part arrived (ADR 0023 §3): a
 *   failed one delivers nothing, because `split` is a partition and a subset of the parts is
 *   not a partition of anything.
 * @property {string} [originalBytes] What the input weighed, as `compress` measured it.
 * @property {string} [producedBytes] What burrow's own version weighed.
 *
 *   BOTH ARE STRINGS BECAUSE THEY ARE `u64`, the same reason `requested` and `allowed` are.
 *
 *   They ride on the reply so the page can say "already efficiently stored" without a second
 *   round trip, and `producedBytes` is the ONLY record of what the rewrite weighed on the
 *   outcome where it is discarded -- `compress` returns two counts rather than a copy of the
 *   input (ADR 0025 §3), so there is no document left to measure.
 *
 *   ABSENT ON EVERY OTHER OPERATION, hence optional: nothing but `compress` computes them.
 * @property {Blob | null} [output] The document an operation produced, or null.
 *
 *   A Blob rather than bytes, for the same reason the INPUT is one (ADR 0015 §4):
 *   structured clone passes it by reference, so the main thread never materialises a
 *   merged document in its own heap. It holds a handle it can turn into a download and
 *   then release.
 */

/**
 * How long after `max_duration_ms` the watchdog waits before killing the worker.
 *
 * The worker enforces `max_duration_ms` itself, at checkpoints, and reports a typed
 * `LimitExceeded` when it can. This is the outer bound for when it *cannot* — a single engine
 * call that never returns, which ADR 0007 says checkpoint-based enforcement cannot interrupt.
 *
 * The grace covers the ack message's trip to the page, the reply's trip back, and the timer's
 * own resolution. Half a second is generous for all three and negligible against any
 * `max_duration_ms` a caller would set.
 */
export const WATCHDOG_GRACE_MS = 500;

/**
 * How long start-up may go **without news** before the worker is declared dead.
 *
 * **Separate from `max_duration_ms`, and that is the point.** Start-up fetches and compiles
 * every wasm module and runs the CSP guard's probe. That was 6.8 MB across three modules
 * until spike 0004 took PDFium out of the payload; it is now 1,670,307 bytes across two — the
 * sum of the `.wasm` lines in `apps/web/size-budget.json` (1,494,731 + 175,576). Charging
 * that to
 * the file would tell a user their document took too long when what was slow was the page —
 * the web form of the queue-time bug PR 2 fixed on the native path, where a caller delayed
 * behind someone else's work was told its own deadline had expired.
 *
 * # It bounds silence, not duration, and it took a measurement to learn why
 *
 * This was 60 s and bounded the WHOLE of start-up, with a comment claiming that was
 * "comfortably above any measured cold load". Chrome's own "Slow 3G" profile refuted it: the
 * engines take over two minutes there, so the first file a person chose on a slow connection
 * was refused with "Something inside burrow failed", the worker was discarded **as a crash**,
 * and three attempts would have latched the circuit breaker and taken the page offline. On a
 * slow connection the engines were never going to load at all.
 *
 * So the worker says so as each module lands (`prelude.js`) and this timer starts again. What
 * is bounded is a worker that has gone quiet, which is the hang the watchdog is actually for.
 *
 * # The number is a floor bandwidth, stated rather than guessed
 *
 * One message per module is the finest granularity available — `integrity` makes the browser
 * verify a whole response before releasing any of it, so a module's body arrives in one read
 * however long it took. The gap this must tolerate is therefore **one module's download**, and
 * the largest is now `qpdf.wasm` at 1,494,731 bytes — it was `pdfium.wasm` at 5,315,922 until
 * spike 0004, so the module this bound is derived from shrank by 3.6×.
 *
 * 240 s allows that module at about 6 KB/s, or 50 kbps — far below Chrome's "Slow 3G" preset
 * (400 kbps). A connection slower than that for four minutes together is not one this page can
 * serve anyway.
 *
 * **NOT LOWERED TO MATCH, and that is deliberate rather than an oversight.** The bound is what
 * a start-up may go *without news*, and the thing it protects against — a person on a slow
 * connection being told their document took too long — got no less real when the payload got
 * smaller. `src/host/start-up-bound.test.ts` derives the floor bandwidth from whatever the
 * largest module currently is, so the slack is reported rather than assumed. Tightening it is
 * a decision with a measurement behind it, not a consequence of this one.
 */
export const DEFAULT_INIT_TIMEOUT_MS = 240_000;

/**
 * How many `starting` messages a start-up may use to push its bound out.
 *
 * One per engine module — `qpdf.wasm` and `burrow_wasm_bg.wasm`, the two `prelude.js` fetches.
 * It was three until spike 0004 took `pdfium.wasm` out of the payload. It is a **cap on
 * trust**, not a count of what must arrive: a start-up that reports fewer still succeeds, and
 * one that reports more is ignored past this point.
 *
 * Stated as a constant rather than hard-coded at the call site because the day an engine is
 * added or removed, the number that has to change is this one and the test that pins it —
 * which is how this line came to be edited rather than forgotten.
 */
export const EXPECTED_ENGINE_MODULES = 2;

/**
 * How long a posted operation may go unacknowledged before the worker is declared dead.
 *
 * **Not the start-up bound, and the difference is the whole of it.** This window opens after
 * `ensureWorker()` has resolved, so the engines are already fetched and compiled; all that is
 * left is the worker's message loop reaching a request and posting `{ ack: true }` before it
 * touches the file. Nothing here depends on anybody's network.
 *
 * It shared `DEFAULT_INIT_TIMEOUT_MS` until that number went to 240 s for a reason belonging
 * entirely to start-up (ADR 0018). 60 s is what it was, and it is generous for a message
 * loop: the alternative is a person watching a spinner for four minutes because a worker died
 * silently, with the crash counted four minutes late.
 */
export const DEFAULT_ACK_TIMEOUT_MS = 60_000;

/** Respawns within {@link DEFAULT_BREAKER_WINDOW_MS} before the breaker opens. */
export const DEFAULT_BREAKER_RESPAWNS = 3;

/**
 * The kind a queued request carries when the page cancelled before it started.
 *
 * Its own kind, like `ENGINE_UNAVAILABLE`, and for the same reason: it is a HOST verdict
 * rather than an `Error` variant, and it is not fatal -- nothing ran, so no instance was
 * poisoned.
 */
export const CANCELLED = "Cancelled";

/** The circuit breaker's sliding window. */
export const DEFAULT_BREAKER_WINDOW_MS = 60_000;

/**
 * A failure the host produced, in the same shape the worker's replies have.
 *
 * One vocabulary. A user who hits a wall should not meet two, and a caller should not have to
 * know whether a given outcome was decided in Rust, in the worker, or here.
 *
 * @param {string} kind
 * @param {string} message
 * @param {{ limit?: string, stage?: string, requested?: string, allowed?: string }} [detail]
 * @returns {HostReply}
 */
function hostFailure(kind, message, detail = {}) {
  return {
    ok: false,
    kind,
    // "This result poisons the engine instance" — which is false for `EngineUnavailable`,
    // where the point is that there is no instance and none will be made. Saying `true`
    // there was not merely imprecise: it made a test that expected a crash pass on a refusal,
    // because both looked fatal.
    fatal: kind !== ENGINE_UNAVAILABLE && kind !== CANCELLED,
    failedInput: -1,
    innerKind: "",
    // Explicitly null, not absent. A caller that reads `reply.output` on a failure should
    // get the same shape it gets from the worker -- "no document" -- rather than
    // `undefined`, which reads as "this reply does not have that concept".
    output: null,
    message,
    pages: 0,
    limit: detail.limit ?? "",
    stage: detail.stage ?? "",
    requested: detail.requested ?? "0",
    allowed: detail.allowed ?? "0",
    recycle: false,
    qpdfHeapBytes: "0",
  };
}

/**
 * A success in the same shape, for the one reply the host produces itself: engine init.
 *
 * @returns {HostReply}
 */
function hostSuccess() {
  return {
    ok: true,
    kind: "",
    fatal: false,
    message: "",
    pages: 0,
    limit: "",
    stage: "",
    requested: "0",
    allowed: "0",
    recycle: false,
    qpdfHeapBytes: "0",
  };
}

/**
 * The host-level verdict that no worker will be provided.
 *
 * **Not an `Error` variant, and deliberately not one.** Every `kind` a reply can otherwise
 * carry is a classification of something an engine returned, computed in Rust by
 * `burrow-wasm`'s `kind_of` — ADR 0009 forbids a binding classifying an engine error, and
 * this file is subject to the same rule. `EngineUnavailable` classifies nothing about any
 * file: it describes the page's own willingness to hand out another worker after a run of
 * crashes, which is a fact only the page has.
 */
export const ENGINE_UNAVAILABLE = "EngineUnavailable";

/**
 * Build a worker host.
 *
 * @param {object} options
 * @param {() => WorkerLike} options.spawn Create one worker. Called once per generation.
 * @param {() => void} [options.release] Release whatever `spawn` holds. Called by `dispose()`.
 * @param {() => number} options.now Milliseconds, monotonic. `performance.now()` in a browser.
 * @param {(fn: () => void, ms: number) => unknown} options.setTimer
 * @param {(handle: unknown) => void} options.clearTimer
 * @param {number} [options.maxDurationMs] The operation deadline the watchdog backs up.
 * @param {number} [options.initTimeoutMs]
 * @param {number} [options.ackTimeoutMs]
 * @param {number} [options.breakerRespawns]
 * @param {number} [options.breakerWindowMs]
 */
export function createWorkerHost(options) {
  const {
    spawn,
    release,
    now,
    setTimer,
    clearTimer,
    maxDurationMs = 30_000,
    initTimeoutMs = DEFAULT_INIT_TIMEOUT_MS,
    ackTimeoutMs = DEFAULT_ACK_TIMEOUT_MS,
    breakerRespawns = DEFAULT_BREAKER_RESPAWNS,
    breakerWindowMs = DEFAULT_BREAKER_WINDOW_MS,
  } = options;

  /** @type {HostState} */
  let state = "dead";
  /** @type {WorkerLike | null} */
  let worker = null;

  /**
   * The current worker's generation.
   *
   * A BACKSTOP, and the comment says so because a mutation sweep proved it: with the
   * generation check disabled, the whole suite in `worker-host.test.ts` still passes. Three
   * things independently stop a late message from a terminated worker, and this is the third
   * of them —
   * `discard()` nulls the handler before terminating, and the request was removed from
   * `pending` when it was settled.
   *
   * It is kept rather than deleted because it is the only one of the three that survives a
   * refactor forgetting the other two, and because what it guards against is silent: a
   * result arriving after the watchdog fired, settling a request that belongs to the
   * worker's successor. But it is not what is doing the work today, and a reader should not
   * have to discover that by experiment.
   */
  let generation = 0;

  /**
   * Restart the start-up stall timer, or nothing if no start-up is in flight.
   *
   * Reassigned by `ensureWorker` for the initialisation it is running, and reset to a no-op
   * once that settles -- so a stray `starting` message from a worker that has finished, or
   * from one that has been discarded, cannot push a bound out.
   */
  let rearmStartup = () => {};

  /** How many workers have ever been spawned. The recovery tests read this. */
  let spawnCount = 0;

  /**
   * In-flight requests on the CURRENT generation, by id.
   *
   * @type {Map<number, { settle: (reply: HostReply) => void, timer: unknown, budgetMs: number,
   *   isInit: boolean, acked: boolean, parts?: Blob[], partsExpected?: number,
   *   onProgress?: (progress: { part: number, of: number }) => void }>}
   */
  let pending = new Map();

  let nextId = 1;

  /** The spawn-and-initialise currently in progress, so a second request joins it. */
  /** @type {Promise<boolean> | null} */
  let transition = null;

  /**
   * The tail of the operation queue.
   *
   * **Operations are serialised, because the worker is.** Its message loop is single-threaded,
   * so two concurrent `run()`s meant the second sat unacked behind the first — and the pre-ack
   * timer (which bounds *start-up*, at 60 s) would then fire on the first, terminate a healthy
   * worker mid-operation, report `Internal` to a caller whose own budget had minutes left, and
   * count a crash towards the breaker. Three of those and the page refuses to work at all.
   *
   * That is the same class of bug the ack exists to prevent — time spent waiting for the worker
   * charged to whoever happens to be holding the request. The ack removed it from the window
   * *after* the worker takes the operation; this removes it from the window before.
   *
   * Queue position is not a deadline. A request waiting here carries no timer, deliberately:
   * nothing about it is the file's fault yet, and `discardWorker()` is how a page cancels.
   *
   * @type {Promise<unknown>}
   */
  let queue = Promise.resolve();

  /**
   * How many times the page has cancelled.
   *
   * `discard()` settles the requests that are IN FLIGHT. It does not reach the ones still in
   * `queue` — they have not been posted, so there is nothing to settle — and without this
   * counter each of them went on to run after the cancel: `ensureWorker()` found the state
   * `dead`, the breaker permitted a spawn because a page-initiated discard is not a crash,
   * and a fresh worker recompiled every engine to finish an operation the person had
   * already stopped. The UI had returned to idle and said "Stopped."; the work had not.
   *
   * So a request records the count when it is ENQUEUED, and abandons itself if the count has
   * moved by the time the queue reaches it. That is the difference between stopping the work
   * and stopping the reporting of it.
   */
  let cancellations = 0;

  /**
   * Timestamps of recent **crashes**, pruned to the breaker's window.
   *
   * CRASHES, not respawns, and the distinction is not pedantry — counting respawns was the
   * first implementation and it was wrong. A recycle is a respawn, and so is a page-initiated
   * discard, so a page working through a run of large files would recycle three times and
   * take itself offline with `EngineUnavailable`. Measured: `e2e/measure.spec.ts` recorded
   * two of its five respawn samples as `0ms` because the breaker had already opened.
   *
   * The breaker exists to stop a loop in which a hostile file destroys worker after worker.
   * Only an involuntary death — a fatal reply, a watchdog kill, a worker error, or a failed
   * start-up — is evidence of that. A recycle is the system working.
   *
   * @type {number[]}
   */
  let crashes = [];

  /** Whether the breaker has tripped. Only `reset()` closes it. */
  let breakerOpen = false;

  let disposed = false;

  /**
   * What the worker reported for `MIN_CONVERGING_MEMORY_BYTES`, as a string.
   *
   * Carried from Rust rather than duplicated here. See `min_converging_memory_bytes` in
   * `bindings/burrow-wasm`: a page choosing limits for a constrained device needs the floor
   * below which every operation costs a respawn, and a second definition in JavaScript would
   * be free to drift from the one that decides.
   */
  let minConverging = "";

  /**
   * `Limits::DEFAULT`, as Rust reports it.
   *
   * A caller that wants the core's ceilings must get the core's ceilings, not whatever the page
   * chose — the conformance harness in particular, where the native side takes an omitted
   * `limits` block literally and the two sides must run under the same numbers.
   *
   * @type {Record<string, number> | null}
   */
  let coreDefaults = null;

  // ---------------------------------------------------------------------------------
  // Teardown
  // ---------------------------------------------------------------------------------

  /**
   * Discard the current worker and fail everything on it.
   *
   * **Never retries.** ADR 0009: a retry against a poisoned instance is worse than a visible
   * failure, and the watchdog joins that rule for the same reason — a file that hangs one
   * worker hangs the next.
   *
   * @param {string} kind
   * @param {string} message
   * @param {object} [options]
   * @param {boolean} [options.crash] Whether this death counts towards the circuit breaker.
   * @param {Map<number, HostReply>} [options.overrides] Ids whose failure differs from the
   *   default.
   */
  function discard(kind, message, options = {}) {
    if (options.crash) {
      crashes.push(now());
      // Evaluated HERE, not lazily at the next spawn attempt. A page showing a banner polls
      // `breakerOpen()`; if the verdict only materialised when something next tried to spawn,
      // the accessor would read `false` in exactly the moment the user needs to be told
      // otherwise. `run()` behaves identically either way — this makes the accessor honest.
      pruneCrashes();
      if (crashes.length >= breakerRespawns) {
        breakerOpen = true;
      }
    }
    const dying = worker;
    const doomed = pending;

    // Swap the state out BEFORE terminating and before settling anything. A `settle`
    // callback can re-enter this module synchronously (a caller's `.then` runs later, but a
    // test's does not have to), and it must find a host that has already moved on.
    worker = null;
    pending = new Map();
    generation += 1;
    state = "dead";

    if (dying) {
      dying.onmessage = null;
      dying.onerror = null;
      dying.terminate();
    }

    for (const [id, entry] of doomed) {
      clearTimer(entry.timer);
      entry.settle(options.overrides?.get(id) ?? hostFailure(kind, message));
    }
  }

  // ---------------------------------------------------------------------------------
  // Spawning
  // ---------------------------------------------------------------------------------

  /**
   * Whether the breaker permits another spawn.
   *
   * A sliding window: crashes older than `breakerWindowMs` are forgotten, so an occasional
   * failure over a long session never trips it, while a hostile file destroying three workers
   * in a minute does.
   */
  function allowSpawn() {
    pruneCrashes();
    if (crashes.length >= breakerRespawns) {
      breakerOpen = true;
      return false;
    }
    return true;
  }

  /** Forget crashes older than the window. */
  function pruneCrashes() {
    crashes = crashes.filter((at) => at > now() - breakerWindowMs);
  }

  /**
   * Spawn a worker and run engine init on it. Resolves whether it is usable.
   *
   * @returns {Promise<boolean>}
   */
  function startWorker() {
    // The first worker is `initialising`; a replacement is `respawning`. Same work, and two
    // names because "a request arrived during a respawn" has to be an observable state.
    state = spawnCount === 0 ? "initialising" : "respawning";

    spawnCount += 1;
    const mine = generation;
    const w = spawn();
    worker = w;

    w.onmessage = (event) => {
      if (generation !== mine) {
        // A message from a superseded worker. Dropped in silence: the request it belongs to
        // was settled when its worker was discarded, and there is nothing here to report.
        // See `generation` — unreachable today, and deliberately still here.
        return;
      }
      handleMessage(event.data);
    };

    w.onerror = (event) => {
      if (generation !== mine) {
        return;
      }
      // The event's message is never read or forwarded. It can carry module output, and
      // ADR 0009 says never to echo it. `preventDefault` stops the browser reporting it to
      // the console, which is the one place file-derived bytes must never reach.
      event.preventDefault();
      discard("Internal", "worker terminated", { crash: true });
    };

    return new Promise((resolve) => {
      const id = nextId++;
      let settled = false;

      const finish = (/** @type {boolean} */ ok) => {
        if (settled) return;
        settled = true;
        resolve(ok);
      };

      // A STALL TIMER, NOT A TRANSFER TIMER, and the difference is the whole point of it.
      //
      // It bounded the WHOLE of start-up until M1's consolidation batch, at 60 s, with a
      // comment claiming that was "comfortably above any measured cold load". A measurement
      // refuted it: the engines at 400 kbps -- Chrome's own "Slow 3G" -- were 140
      // seconds of network before anything is compiled, so the first file a person chose on
      // a slow connection was refused at 60 s with "Something inside burrow failed", the
      // worker was discarded as a crash, and three attempts would have latched the circuit
      // breaker and taken the page offline entirely. On a slow connection the engines were
      // never going to load at all.
      //
      // No fixed bound can be right here, because the thing being bounded is somebody else's
      // network. What a watchdog can honestly ask is whether anything is still HAPPENING, so
      // the worker says so as each engine module lands and this timer starts again. A worker
      // that has gone quiet for `initTimeoutMs` is hung; one that is still receiving bytes is
      // working, however slowly.
      const armStallTimer = () =>
        setTimer(() => {
          if (generation !== mine) return;
          // START-UP failing is not the file's fault, so this is `Internal` and NEVER a
          // duration limit. Reporting it as `LimitExceeded` would tell a user to shrink a
          // document that was never looked at.
          discard("Internal", "engine start-up stalled", { crash: true });
        }, initTimeoutMs);

      const timer = armStallTimer();

      // RE-ARMS ARE COUNTED, because an uncounted one hands the watchdog to the thing it is
      // watching. Found by security review: with no cap, a worker posting `starting` on a
      // timer keeps the bound alive forever, `finish` never runs, the breaker never latches,
      // and the page shows "getting the engine ready" until someone reloads -- the exact hang
      // the bound exists to close, reached through the mechanism that was supposed to close
      // it. Not reachable from file content (nothing on this path has seen a file, and the
      // bundle is integrity-pinned), but a watchdog whose limit is set by its subject is not
      // a watchdog.
      //
      // The expected count is KNOWN -- one per engine module -- which is what `CLAUDE.md`
      // asks a gate to use when the number is derivable rather than settling for "non-zero".
      // A fourth message is a bundle doing something this host does not model, so it is
      // ignored rather than obeyed.
      let rearms = 0;

      // Held so a progress message can find it without going through `pending`, which is
      // keyed by request id and this is not a request.
      rearmStartup = () => {
        // THREE GUARDS AND EACH ONE IS LOAD-BEARING, which is worth stating because a fourth
        // was here and was not: `finish` also reset this closure to a no-op, and deleting
        // that line broke no test. Code review mutated it to find that out. It was
        // unreachable behind the two below, and a defence with no failing mutation is a
        // comment rather than a defence.
        //
        //   `generation` -- a message from a worker that has been superseded;
        //   `settled`    -- a message arriving after start-up finished;
        //   `pending`    -- `discard` swaps in a fresh map, so a discarded start-up's entry
        //                   is gone even if the two above somehow passed.
        if (generation !== mine || settled) return;
        if (rearms >= EXPECTED_ENGINE_MODULES) return;
        const entry = pending.get(id);
        if (!entry) return;
        rearms += 1;
        clearTimer(entry.timer);
        entry.timer = armStallTimer();
      };

      pending.set(id, {
        timer,
        budgetMs: maxDurationMs,
        // An INIT entry, marked so an ack can never apply to it. Without the flag, an `ack`
        // carrying the init id would replace the START-UP bound with the operation budget and
        // then report a start-up failure as `LimitExceeded / max_duration_ms` — the exact
        // inversion this design forbids. `main.js` never acks an init; the flag is what makes
        // that a property of the host rather than of the bundle.
        isInit: true,
        acked: false,
        settle: (reply) => {
          if (reply.ok === true) {
            state = "idle";
            finish(true);
            return;
          }
          finish(false);
        },
      });

      w.postMessage({ type: "init", id });
    });
  }

  /**
   * Ensure a usable worker exists, spawning one if the breaker allows.
   *
   * A request arriving while a spawn is in progress **joins that spawn**. It never starts a
   * second one, which `spawnCount()` is what proves.
   *
   * @returns {Promise<boolean>}
   */
  function ensureWorker() {
    if (state === "idle" || state === "busy") {
      return Promise.resolve(true);
    }
    if (transition) {
      return transition;
    }
    if (breakerOpen || !allowSpawn()) {
      return Promise.resolve(false);
    }

    transition = startWorker().finally(() => {
      transition = null;
    });
    return transition;
  }

  // ---------------------------------------------------------------------------------
  // Messages
  // ---------------------------------------------------------------------------------

  /**
   * One message from the current worker.
   *
   * Four shapes arrive here and the type says so rather than reaching for `any`, which
   * `apps/web/CLAUDE.md` forbids: an operation reply (a full `HostReply` plus its id), an
   * `ack`, an init answer carrying `ready`, and a `starting` notice from an engine module
   * that has landed. Everything read below is named.
   *
   * @param {Partial<HostReply> & { id?: number, ack?: boolean, ready?: boolean,
   *   starting?: boolean,
   *   minConvergingMemoryBytes?: string,
   *   defaultLimits?: Record<string, number>,
   *   part?: { index: number, of: number }, progress?: { part: number, of: number },
   *   }} data
   */
  function handleMessage(data) {
    // PROGRESS, which carries no id because it belongs to start-up rather than to a request.
    // It says one thing -- an engine module has landed -- and it is the only evidence the
    // stall timer has that a slow transfer is a transfer rather than a hang.
    if (data.starting === true) {
      rearmStartup();
      return;
    }

    const entry = data.id === undefined ? undefined : pending.get(data.id);
    if (!entry) {
      // A reply to a request that is no longer in flight — the watchdog fired and settled it
      // already, or a worker was discarded between the post and the reply. Dropped: the
      // caller has its answer, and settling twice would be worse than saying nothing.
      return;
    }

    // THE ACK. The watchdog's clock starts here, not when the request was posted. Everything
    // before it — the worker's policy guard, and a cold engine compile — is start-up, which
    // `initTimeoutMs` bounds separately.
    const id = /** @type {number} */ (data.id);

    // A PART, or progress within a multi-output operation (ADR 0023). Neither settles the
    // request: the terminal reply still comes, and it is what the caller is waiting on.
    //
    // THE PARTS ARE HELD, NOT DELIVERED. ADR 0023 §3: a split that fails on part 3 of 10
    // delivers nothing, because `split` is defined as a partition and a subset of the parts is
    // not a partition of anything. So they accumulate here and the terminal reply decides
    // whether the caller ever sees them -- which is also why the watchdog is re-armed per part
    // rather than left running from the ack: a fifty-way split of a large document is fifty
    // units of work, and one budget for all of them would fire on the honest case.
    if (data.part !== undefined) {
      entry.parts = entry.parts ?? [];
      entry.partsExpected = data.part.of;
      if (!data.output) {
        // A PART WITH NO DOCUMENT. The worker posts bytes with every part or posts a failure
        // instead, so this is the bundle doing something this host does not model -- and the
        // gap-check on the terminal reply would catch it anyway. Failing here names it
        // immediately rather than as "a part did not arrive" several messages later.
        entry.settle(hostFailure("Internal", "a part of the split carried no document"));
        clearTimer(entry.timer);
        pending.delete(id);
        // AND THE WORKER GOES. `apps/web/CLAUDE.md`: any `Internal` result is fatal to the
        // worker -- terminate it and spawn a fresh one. These two paths only fire when the
        // bundle violates the protocol, and leaving a worker mid-split holding a live
        // `SplitSession` for a request the host has forgotten is the state that wedges it:
        // `state` would stay "busy" for the life of the host. Every other failure path here
        // discards; these two were the exception. Found by code review.
        discard("Internal", "worker discarded", { crash: true });
        return;
      }
      entry.parts[data.part.index] = data.output;
      if (entry.acked) {
        clearTimer(entry.timer);
        entry.timer = setTimer(() => {
          watchdogFired(id, entry.budgetMs);
        }, entry.budgetMs + WATCHDOG_GRACE_MS);
      }
      return;
    }

    if (data.progress !== undefined) {
      entry.onProgress?.(data.progress);
      return;
    }

    if (data.ack === true) {
      // ONCE, and never for an init. A worker that re-acked would push its deadline out
      // indefinitely and the operation budget would stop existing — a hang with no recovery,
      // which is precisely what the watchdog is here to close. `main.js` acks exactly once per
      // request and its source is integrity-pinned, so neither case is reachable from a file;
      // both are refused here so the guarantee belongs to the host rather than to the bundle.
      if (entry.isInit || entry.acked) {
        return;
      }
      entry.acked = true;
      clearTimer(entry.timer);
      entry.timer = setTimer(() => {
        watchdogFired(id, entry.budgetMs);
      }, entry.budgetMs + WATCHDOG_GRACE_MS);
      return;
    }

    clearTimer(entry.timer);
    pending.delete(id);

    // An init reply. `ready: true` is the only success; `startWorker`'s settle reads it.
    if (typeof data.ready === "boolean") {
      if (data.minConvergingMemoryBytes !== undefined) {
        minConverging = data.minConvergingMemoryBytes;
      }
      if (data.defaultLimits !== undefined) {
        coreDefaults = data.defaultLimits;
      }
      entry.settle(
        data.ready ? hostSuccess() : hostFailure("Internal", "engines failed to initialise"),
      );
      if (!data.ready) {
        discard("Internal", "engines failed to initialise", { crash: true });
      }
      return;
    }

    // A MULTI-OUTPUT REPLY. The terminal message carries no bytes of its own -- the parts came
    // separately -- so the caller is handed them here, and ONLY on success.
    if (entry.partsExpected !== undefined) {
      const held = entry.parts ?? [];
      if (data.ok === true) {
        // EVERY PART, counted against the total the worker declared before the first one.
        // A gap would mean a part message was dropped between the worker and here, which is
        // not something the terminal reply can see -- and delivering nine of ten parts as a
        // success is the outcome ADR 0023 §3 refuses.
        // COUNTED, NOT `every`. `Array.prototype.every` SKIPS HOLES, so an array of length 10
        // whose only assigned element is index 9 passed this gate -- and the parts were
        // delivered as a success containing nine `undefined`s. The comment above says the gate
        // exists because a dropped part message "is not something the terminal reply can see",
        // and that was the one case it could not see either. Measured by security review.
        let arrived = 0;
        for (let i = 0; i < entry.partsExpected; i += 1) {
          if (held[i] instanceof Blob) arrived += 1;
        }
        const complete = arrived === entry.partsExpected && held.length === entry.partsExpected;
        if (!complete) {
          entry.settle(hostFailure("Internal", "a part of the split did not arrive"));
          discard("Internal", "worker discarded", { crash: true });
          return;
        }
        data.parts = held;
      } else {
        // DISCARDED. The caller never sees a part of a failed split.
        entry.parts = undefined;
      }
    }

    if (pending.size === 0 && state === "busy") {
      state = "idle";
    }

    const reply = /** @type {HostReply} */ (data);

    // ADR 0009: `fatal` was computed in Rust. Nothing here re-derives it from `kind`.
    if (reply.fatal) {
      // The caller gets the real reply — it carries the typed outcome and is more useful
      // than the generic `Internal` a discard would substitute — and the instance goes.
      entry.settle(reply);
      discard("Internal", "worker discarded", { crash: true });
      return;
    }

    entry.settle(reply);

    // RECYCLING IS NOT A FAILURE, and the ordering says so: the caller already has its
    // result, and only then does the worker go. A page must never see a successful
    // operation as a lost worker.
    if (reply.recycle === true) {
      // NOT a crash. Recycling is the system working, and counting it would let a page
      // working through large files trip its own breaker.
      discard("Internal", "worker recycled");
    }
  }

  /**
   * The watchdog expired for `id`.
   *
   * The caller that was running gets `LimitExceeded` naming `max_duration_ms`, shaped exactly
   * like the one Rust produces at a checkpoint — same limit name, same three numbers — because
   * the user hit the same wall and should not meet two vocabularies. Anything else in flight
   * gets the ordinary `Internal`.
   *
   * @param {number} id
   * @param {number} budgetMs
   */
  function watchdogFired(id, budgetMs) {
    const overrides = new Map([
      [
        id,
        hostFailure("LimitExceeded", `max_duration_ms exceeded: ${budgetMs}`, {
          limit: "max_duration_ms",
          // The same stage Rust reports when its own checkpoint catches the deadline. The
          // watchdog is a different MECHANISM -- it terminates the worker rather than
          // returning from a checkpoint -- but it is the same ceiling and the same question,
          // and a caller should not have to know which side noticed.
          stage: "deadline",
          requested: String(budgetMs + WATCHDOG_GRACE_MS),
          allowed: String(budgetMs),
        }),
      ],
    ]);
    discard("Internal", "worker discarded", { crash: true, overrides });
  }

  /**
   * One operation, once the queue reaches it.
   *
   * @param {object} message
   * @param {{ maxDurationMs?: number,
   *   onProgress?: (progress: { part: number, of: number }) => void }} options
   * @returns {Promise<HostReply>}
   */
  async function runOne(message, options) {
    if (disposed) {
      return hostFailure("Internal", "host disposed");
    }

    const usable = await ensureWorker();
    if (!usable) {
      if (breakerOpen) {
        return hostFailure(
          ENGINE_UNAVAILABLE,
          "the engines have crashed repeatedly and will not be restarted automatically",
        );
      }
      return hostFailure("Internal", "no worker");
    }

    const live = worker;
    if (!live) {
      return hostFailure("Internal", "no worker");
    }

    const id = nextId++;
    state = "busy";

    return new Promise((resolve) => {
      // Bounded from the moment it is posted: everything between the post and the ack is the
      // worker getting to this request, not the file being processed. Without this a worker
      // that dies before acking leaves the caller waiting for a reply that will never come.
      //
      // It is only ever this request's own wait, because `run` serialises — see `queue`. When
      // it was armed while another operation was still in flight, its expiry killed a healthy
      // worker and counted a crash.
      //
      // BY ITS OWN BOUND, not the start-up one. It shared `initTimeoutMs` until security
      // review noticed what ADR 0018 had just done to that number: raising it 60 s -> 240 s
      // for a 5.3 MB download silently also quadrupled this window, which contains no
      // download at all -- it opens only after `ensureWorker()` has resolved, so the engines
      // are already compiled. A worker that died silently after init would have left a person
      // watching a spinner for four minutes, with the crash counted four minutes late. Two
      // bounds sharing a constant because they once had the same value is how a justified
      // number becomes an unjustified one.
      const timer = setTimer(() => {
        discard("Internal", "worker did not accept the operation", { crash: true });
      }, ackTimeoutMs);

      pending.set(id, {
        settle: resolve,
        timer,
        budgetMs: options.maxDurationMs ?? maxDurationMs,
        isInit: false,
        acked: false,
        onProgress: options.onProgress,
      });
      live.postMessage({ ...message, id });
    });
  }

  // ---------------------------------------------------------------------------------
  // The public surface
  // ---------------------------------------------------------------------------------

  return {
    /** The current state. @returns {HostState} */
    state: () => state,

    /** How many workers have ever been spawned. */
    spawnCount: () => spawnCount,

    /** Whether a live worker is held. */
    hasWorker: () => worker !== null,

    /**
     * The smallest `max_memory_bytes` at which recycling converges, as Rust reports it.
     *
     * A string because it is a `u64`. Empty until a worker has initialised — there is no
     * sensible default, and a page inventing one would be the copy this exists to avoid.
     */
    minConvergingMemoryBytes: () => minConverging,

    /** `Limits::DEFAULT` as Rust reports it, or `null` before a worker has initialised. */
    coreDefaultLimits: () => coreDefaults,

    /** Whether the circuit breaker has tripped. */
    breakerOpen: () => breakerOpen,

    /**
     * Bring the engines up without running an operation.
     *
     * @returns {Promise<boolean>}
     */
    async ready() {
      if (disposed) return false;
      return ensureWorker();
    },

    /**
     * Run one operation.
     *
     * `message` carries the `Blob` — never a transferred `ArrayBuffer`. A Blob is passed by
     * reference under structured clone, so the main thread never materialises the file bytes,
     * and the caller still holds a usable handle after a worker is killed. A transferred
     * buffer is detached on the page side and gone, which would make "retry on a fresh
     * worker" impossible for exactly the files that needed it.
     *
     * `maxDurationMs` is per operation, not per host, because `Limits` are: a page may run a
     * 2 MB file and a 200 MB one with different ceilings, and a watchdog fixed at
     * construction would apply the wrong one to the second. It backs up the same
     * `max_duration_ms` the worker is enforcing at its checkpoints, so the two must be the
     * same number.
     *
     * @param {object} message
     * @param {{ maxDurationMs?: number,
     *   onProgress?: (progress: { part: number, of: number }) => void }} [options]
     * @returns {Promise<HostReply>}
     */
    run(message, options = {}) {
      const enqueuedAt = cancellations;
      const mine = queue.then(() =>
        cancellations === enqueuedAt
          ? runOne(message, options)
          : // NOT `Internal`. `Internal` means "this result poisons the engine instance", and
            // a caller acting on it would discard a worker that is perfectly healthy -- or
            // spawn one to be told about an operation nobody is waiting for. Nothing went
            // wrong here; the person changed their mind before this request started.
            hostFailure(CANCELLED, "cancelled before it started"),
      );
      // The chain must survive a rejection, or one failed operation would wedge every later
      // one. `runOne` never rejects — every outcome is a `HostReply` — but a bug there would
      // otherwise be silent and permanent.
      queue = mine.then(
        () => undefined,
        () => undefined,
      );
      return mine;
    },
    /**
     * Close the circuit breaker.
     *
     * Deliberately **not** automatic. "Stop respawning" has to mean stopped: a page that
     * retried on a timer would resume the crash loop at a slower rate rather than end it. A
     * UI wires this to a deliberate user gesture — a "try again" button — so another worker
     * is only ever spent because a person decided to spend it.
     */
    reset() {
      breakerOpen = false;
      crashes = [];
    },

    /**
     * Discard the current worker without disposing the host.
     *
     * What a "cancel" button does, and what a test does when it wants the next request to
     * build a differently-configured worker. Everything in flight fails, and nothing is
     * retried — the same rule every other teardown follows.
     */
    discardWorker() {
      // BEFORE the discard, so a request enqueued during it is still counted as cancelled.
      cancellations += 1;
      discard("Internal", "worker discarded by the page");
    },

    /** Terminate everything and release the factory's resources. */
    dispose() {
      disposed = true;
      discard("Internal", "host disposed");
      release?.();
    },
  };
}
