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

  // The worker's source text, fetched once with its integrity pinned.
  //
  // A worker MUST be created from a Blob here. A worker loaded from a plain same-origin URL
  // does not inherit this page's CSP -- it takes its policy from that script's HTTP response
  // headers, and a static host sends none, so it would run unpoliced. Measured in M1 PR
  // 4a-i; see docs/adr/0014-web-engine-loading-and-csp.md. The worker itself refuses to
  // touch a file unless `self.location.protocol === "blob:"`, so getting this wrong fails
  // closed rather than silently dropping the browser-enforced half of the guarantee.
  let workerUrl = null;
  async function workerObjectUrl() {
    if (workerUrl) {
      return workerUrl;
    }
    const entry = ENGINES.worker;
    const response = await fetch(entry.url, { integrity: entry.integrity });
    if (!response.ok) {
      throw new Error("worker fetch failed");
    }
    const source = await response.text();
    workerUrl = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
    return workerUrl;
  }

  function spawn(objectUrl) {
    spawnCount += 1;
    const w = new Worker(objectUrl);
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

  async function send(message, transfer) {
    const objectUrl = await workerObjectUrl();
    worker ??= spawn(objectUrl);
    const id = nextId++;
    const live = worker;
    return new Promise((resolve) => {
      pending.set(id, resolve);
      // No `engines` in the message: the manifest is generated into the worker bundle,
      // because the Emscripten glue starts instantiating as it is parsed and there is no
      // moment after load at which a message could arrive in time.
      live.postMessage({ ...message, id }, transfer);
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

    /**
     * Try to fetch `url` from INSIDE a worker, and report whether the browser refused.
     *
     * The page-level equivalent proves the policy applies to the document. This proves it
     * applies where the file bytes actually are — the page never sees them after the
     * transfer, so a policy covering only the document would be the wrong way round and
     * would look identical from outside.
     */
    async fetchFromWorker(url) {
      // A probe worker built the same way the real one is: from a Blob, so it inherits this
      // page's policy. Constructing it from a URL instead would give it no policy, and the
      // test would report "not blocked" for the wrong reason -- which is precisely the bug
      // this API exists to detect.
      //
      // `securitypolicyviolation` is reported alongside the outcome because "the fetch
      // failed" and "the browser refused the fetch" are different facts. A network error
      // reaching an unreachable port looks identical from the catch block; only the event
      // says the policy is what stopped it.
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
          const done = (value) => {
            probe.terminate();
            resolve(value);
          };
          probe.onmessage = (event) => done(event.data);
          // A worker that cannot start has not made the request either, but that is a
          // different fact from "the fetch was blocked". Report it as not-blocked so a test
          // fails loudly rather than passing for the wrong reason.
          probe.onerror = () => done({ blocked: false, violations: [], failed: true });
          probe.postMessage({ url });
        });
      } finally {
        URL.revokeObjectURL(probeUrl);
      }
    },

    /** The worker's own view of whether it inherits this page's CSP. */
    async workerInheritsCsp() {
      const reply = await send({ type: "init" });
      return reply.ready === true;
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
