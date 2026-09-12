// A fake worker for the host's unit tests, and a deliberate lesson from PR 4a-i.
//
// WHY THIS TRACKS RESOURCES
//
// 4a-i's Rust fake bridge returned a **fixed handle** from `logger_create()`. That is the
// obvious way to write a fake, and it hid a real leak: the web qpdf path created a logger per
// operation and never released it, into a heap that never shrinks. The fake could not have
// noticed, because a fixed handle models an object that is never allocated in the first place.
// The fix was to make the fake *account* for what the real thing hands out.
//
// The same trap is here in a different shape. A fake worker that is a bag of callbacks cannot
// notice a worker that is never terminated, a request that is never settled, or an object URL
// that is never revoked — and every one of those is a leak the host is supposed to prevent. So
// this fake tracks every instance it creates, every message it received, whether `terminate()`
// was called on each instance exactly once, and whether the factory's own resource was
// released. `assertNoLeaks()` is what makes those observable, and the tests call it after
// every case.
//
// It is TEST SUPPORT and is never staged into `public/`.

/**
 * @typedef {object} FakeInstance
 * @property {number} generation
 * @property {unknown[]} received Every message posted to this instance.
 * @property {number} terminations How many times `terminate()` was called. Must end at 1.
 * @property {(message: unknown) => void} reply Deliver a message to the host from this worker.
 * @property {() => void} crash Fire `onerror`, as a browser-killed worker does.
 * @property {(message: unknown) => void} postMessage
 * @property {() => void} terminate
 * @property {((event: MessageEvent) => void) | null} onmessage
 * @property {((event: ErrorEvent) => void) | null} onerror
 */

/**
 * A factory of fake workers, with the bookkeeping the host's invariants need.
 *
 * @param {object} [options]
 * @param {(instance: FakeInstance, message: any) => void} [options.onMessage]
 *   Called for every message the host posts. The default answers `init` with `ready: true` and
 *   leaves operations hanging, which is the shape most tests want: they drive the reply
 *   themselves.
 */
export function createFakeWorkerFactory(options = {}) {
  /** @type {FakeInstance[]} */
  const instances = [];
  let released = 0;

  const onMessage =
    options.onMessage ??
    ((instance, message) => {
      if (message?.type === "init") {
        instance.reply({ id: message.id, ready: true });
      }
    });

  function spawn() {
    /** @type {FakeInstance} */
    const instance = {
      generation: instances.length,
      received: [],
      terminations: 0,
      onmessage: null,
      onerror: null,

      postMessage(message) {
        instance.received.push(message);
        // Asynchronous, like a real worker. A fake that answered synchronously would let a
        // host with a re-entrancy bug pass, because the reply would arrive before the caller
        // had finished setting up.
        queueMicrotask(() => onMessage(instance, message));
      },

      terminate() {
        instance.terminations += 1;
      },

      reply(message) {
        // Cast rather than constructed: `MessageEvent` is a DOM class and the host reads
        // exactly one property off it. Building a real one would need a DOM, which is the
        // thing these tests exist not to need.
        instance.onmessage?.(/** @type {MessageEvent} */ ({ data: message }));
      },

      crash() {
        instance.onerror?.(
          /** @type {ErrorEvent} */ (/** @type {unknown} */ ({ preventDefault() {} })),
        );
      },
    };
    instances.push(instance);
    return instance;
  }

  return {
    spawn,

    /** What `dispose()` calls. Stands in for `URL.revokeObjectURL`. */
    release() {
      released += 1;
    },

    /** Every instance ever spawned, in order. */
    instances: () => instances,

    /** The most recently spawned instance. */
    latest: () => instances[instances.length - 1],

    /** How many times the factory's resource was released. */
    releases: () => released,

    /**
     * Fail if anything the host was responsible for was left behind.
     *
     * `liveAllowed` is how many instances may legitimately still be running — one while the
     * host holds a worker, zero after `dispose()`. Stating it per call rather than inferring
     * it keeps the assertion honest: a test that expects a live worker says so.
     *
     * @param {number} liveAllowed
     */
    assertNoLeaks(liveAllowed) {
      const live = instances.filter((i) => i.terminations === 0);
      if (live.length !== liveAllowed) {
        throw new Error(
          `expected ${liveAllowed} live worker(s), found ${live.length} ` +
            `(generations ${live.map((i) => i.generation).join(", ") || "none"})`,
        );
      }
      const doubleKilled = instances.filter((i) => i.terminations > 1);
      if (doubleKilled.length > 0) {
        // Terminating twice is harmless in a browser and is still a bug: it means two code
        // paths both believed they owned the instance, which is how a reply gets delivered to
        // a worker that a different path already replaced.
        throw new Error(
          `worker(s) terminated more than once: ` +
            doubleKilled.map((i) => `${i.generation}×${i.terminations}`).join(", "),
        );
      }
    },
  };
}

/**
 * A controllable clock and timer set, so no test waits on real time.
 *
 * ADR 0007 made the *core's* clock injectable for exactly this reason — "a timeout test that
 * really waits 60 seconds is a test nobody runs". The watchdog is a timeout, so it gets the
 * same treatment.
 *
 * `advance` fires every timer due at or before the new time, in due order, and re-checks after
 * each one: a timer set by a firing callback (which is precisely what the ack does) must be
 * able to fire within the same advance.
 */
export function createFakeClock() {
  let time = 0;
  let nextHandle = 1;
  /** @type {Map<number, { at: number, fn: () => void }>} */
  const timers = new Map();

  return {
    now: () => time,

    /** @param {() => void} fn @param {number} ms */
    setTimer(fn, ms) {
      const handle = nextHandle++;
      timers.set(handle, { at: time + ms, fn });
      return handle;
    },

    /** @param {unknown} handle */
    clearTimer(handle) {
      timers.delete(/** @type {number} */ (handle));
    },

    /** How many timers are outstanding. A leaked timer is a leaked callback. */
    outstanding: () => timers.size,

    /** @param {number} ms */
    advance(ms) {
      const target = time + ms;
      for (;;) {
        /** @type {{ handle: number, at: number, fn: () => void } | null} */
        let due = null;
        for (const [handle, timer] of timers) {
          if (timer.at <= target && (due === null || timer.at < due.at)) {
            due = { handle, at: timer.at, fn: timer.fn };
          }
        }
        if (!due) break;
        timers.delete(due.handle);
        time = due.at;
        due.fn();
      }
      time = target;
    },
  };
}

/**
 * A reply in the shape the worker sends, so a test states only what it is testing.
 *
 * @param {number} id
 * @param {Partial<Record<string, unknown>>} [overrides]
 */
export function workerReply(id, overrides = {}) {
  return {
    id,
    ok: true,
    kind: "",
    fatal: false,
    message: "",
    pages: 1,
    limit: "",
    stage: "",
    requested: "0",
    allowed: "0",
    recycle: false,
    pdfiumHeapBytes: "1048576",
    qpdfHeapBytes: "1048576",
    ...overrides,
  };
}
