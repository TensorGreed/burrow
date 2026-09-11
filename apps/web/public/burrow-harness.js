// The test-only harness driver.
//
// NOT SHIPPED: `astro.config.mjs`'s `burrow:exclude-harness` integration deletes this from
// `dist/` unless `BURROW_HARNESS=1`, and `src/production-build.test.ts` asserts it is gone.
//
// WHY THIS IS AN EXTERNAL FILE AND NOT AN INLINE SCRIPT
//
// It was inline first, and the CSP refused it:
//
//   Executing inline script violates the following Content Security Policy directive
//   'script-src 'self' 'wasm-unsafe-eval''.
//
// That is the policy working. `script-src 'self'` with no `'unsafe-inline'` and no nonce
// means *no* inline script runs, which is a property worth keeping — it is the difference
// between a policy that stops an injected `<script>` and one that does not. So the harness
// moved out of line rather than the policy loosening to accommodate it, and the same
// constraint now applies to every page on the site.
//
// The engine manifest arrives through a `data-` attribute rather than a variable, because
// there is no way to pass one to an external script without inlining something.

"use strict";

(() => {
  const status = document.getElementById("status");
  const result = document.getElementById("result");
  const manifest = document.getElementById("engines");
  const ENGINES = JSON.parse(manifest.dataset.engines);

  let worker = null;
  let nextId = 1;
  let spawnCount = 0;
  const pending = new Map();

  function spawn() {
    spawnCount += 1;
    const w = new Worker("/burrow-worker.js");
    w.onmessage = (event) => {
      const reply = event.data;
      const settle = pending.get(reply.id);
      if (settle) {
        pending.delete(reply.id);
        settle(reply);
      }
      // ADR 0009: the page treats the RESULT as the signal. The worker survives a trap and
      // no error event fires, so nothing forces our hand. `fatal` was computed in Rust;
      // nothing here re-derives it from `kind`.
      if (reply.fatal) {
        w.terminate();
        if (worker === w) worker = null;
        // Everything still in flight on a discarded worker fails, and is not retried: a
        // retry against a poisoned instance is worse than a visible failure.
        for (const [id, settlePending] of pending) {
          settlePending({
            id,
            ok: false,
            kind: "Internal",
            fatal: true,
            message: "worker discarded",
          });
        }
        pending.clear();
      }
    };
    return w;
  }

  function send(message, transfer) {
    worker ??= spawn();
    const id = nextId++;
    return new Promise((resolve) => {
      pending.set(id, resolve);
      worker.postMessage({ ...message, id, engines: ENGINES }, transfer);
    });
  }

  // The API the Playwright tests drive. On `window` deliberately: a test should call the
  // thing under test, not synthesise clicks on a UI that does not exist yet.
  window.burrowHarness = {
    async ready() {
      const reply = await send({ type: "init" });
      return reply.ready === true;
    },

    /** Run one operation over `bytes`, a plain array of byte values from the test. */
    async run(op, bytes, options = {}) {
      const buffer = new Uint8Array(bytes).buffer;
      const password = options.password ? new Uint8Array(options.password).buffer : null;
      const limits = options.limits ?? {
        maxInputBytes: 100 * 1024 * 1024,
        maxMemoryBytes: 1024 * 1024 * 1024,
        maxDurationMs: 30000,
        maxPages: 10000,
        maxPixels: 100000000,
      };
      const transfer = password ? [buffer, password] : [buffer];
      return send(
        {
          op,
          bytes: buffer,
          password,
          limits,
          attemptRecovery: options.attemptRecovery ?? false,
        },
        transfer,
      );
    },

    /** How many workers have been spawned. 4a-ii's recovery test reads this. */
    spawnCount() {
      return spawnCount;
    },

    /** Whether a live worker is currently held. */
    hasWorker() {
      return worker !== null;
    },
  };

  // A visible signal for a human opening the page by hand, and what the tests wait on.
  window.burrowHarness
    .ready()
    .then((ok) => {
      status.textContent = ok ? "engines ready" : "engines failed to initialise";
      result.textContent = JSON.stringify(ENGINES, null, 2);
    })
    .catch(() => {
      status.textContent = "engines failed to initialise";
    });
})();
