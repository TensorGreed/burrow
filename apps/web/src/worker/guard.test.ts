// The worker's fail-closed guard, driven in every state a browser cannot put it in.
//
// `worker-src blob:` means the browser refuses to construct this worker from a URL, so
// `e2e/worker-guard.spec.ts` can no longer reach the guard's refusing branch — which is the
// right trade (the browser stops the mistake earlier), but it leaves the inner layer
// untested unless it is tested here.
//
// The guard's logic is small and its inputs are two globals, so the bundle's `prelude.js` is
// evaluated in `node:vm` against a stubbed worker scope. That is the only way to answer the
// question that matters: **does it refuse when no policy is in force?**
//
// Two earlier versions were wrong, and both are pinned below:
//
//   * `self.location.protocol === "blob:"` alone, named `INHERITS_PAGE_CSP`. A Blob worker
//     inherits the creating document's policy *whatever that is*, including none — so the
//     name asserted something the check did not establish.
//   * Requiring a `securitypolicyviolation` event. **WebKit does not dispatch it in a
//     worker**: it enforced the policy correctly, refused the probe, fired no event, and the
//     guard concluded there was no policy and refused every operation. Measured.
//
// The check is now DIFFERENTIAL: an allowlisted fetch must succeed and a non-allowlisted one
// must be refused. That separates "refused by policy" from "the network is broken" using only
// whether requests succeed, which every browser agrees on.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";

import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const prelude = readFileSync(join(here, "prelude.js"), "utf8");

interface GuardOutcome {
  createdFromBlob: boolean;
  policed: boolean;
}

/**
 * Evaluate the prelude against a stubbed worker scope and report what the guard concluded.
 *
 * @param protocol what `self.location.protocol` reports
 * @param fetchBehaviour how the probe request resolves
 */
async function runGuard(
  protocol: string,
  world: "policy-in-force" | "no-policy" | "network-broken" | "probe-hangs",
): Promise<GuardOutcome> {
  const scope: Record<string, unknown> = {
    location: { protocol },
    addEventListener: () => {},
    removeEventListener: () => {},
    // The manifest the bundle would otherwise have generated in above the prelude.
    BURROW_ENGINES: {
      probeOrigin: "https://example.test",
      pdfiumWasm: { url: "https://example.test/a.wasm", integrity: "sha384-x" },
      qpdfWasm: { url: "https://example.test/b.wasm", integrity: "sha384-y" },
      burrowWasm: { url: "https://example.test/c.wasm", integrity: "sha384-z" },
    },
    WebAssembly: { compileStreaming: async () => ({}), instantiate: async () => ({}) },
    URL,
    fetch: async (url: string) => {
      // The allowlisted engine fetches. These are the CONTROL: the guard requires one to
      // succeed before it will read anything into the probe being refused.
      if (url.endsWith(".wasm")) {
        if (world === "network-broken") {
          throw new TypeError("Failed to fetch");
        }
        return { ok: true };
      }

      // The probe, at a path nothing serves.
      if (world === "no-policy") {
        // No policy: the request goes out and 404s. `fetch` RESOLVES on a 404 — it only
        // rejects on a network-level failure — so this is what "nothing refused it" looks
        // like, and the guard must read it as "not policed".
        return { ok: false, status: 404 };
      }
      if (world === "probe-hangs") {
        return new Promise(() => {});
      }
      // Refused by the policy. No violation event is dispatched, because WebKit does not
      // dispatch one in a worker and the guard must not depend on it.
      throw new TypeError("Refused to connect");
    },
    setTimeout,
    // `clearTimeout` too: without it the guard's listener throws a ReferenceError, the
    // violation is never observed, and the bounded timeout fires instead -- which looks
    // exactly like "no policy" and made this suite fail for a reason that was not the code.
    clearTimeout,
    console,
  };
  scope.self = scope;
  scope.globalThis = scope;

  // The completion value, not properties of the context: `const` in a script evaluated by
  // `vm` is a lexical binding in that script's scope and never becomes a property of the
  // context object, so reading `scope.CREATED_FROM_BLOB` yields `undefined`.
  const result = runInNewContext(`${prelude}\n;({ CREATED_FROM_BLOB, POLICED })`, scope) as {
    CREATED_FROM_BLOB: boolean;
    POLICED: Promise<boolean>;
  };

  return {
    createdFromBlob: result.CREATED_FROM_BLOB,
    policed: await result.POLICED,
  };
}

describe("the worker's fail-closed guard", () => {
  it("accepts a blob: worker whose probe the policy refuses", async () => {
    const outcome = await runGuard("blob:", "policy-in-force");
    expect(outcome.createdFromBlob).toBe(true);
    expect(
      outcome.policed,
      "an allowlisted fetch succeeding while a non-allowlisted one is refused IS a policy",
    ).toBe(true);
  });

  it("does not need a securitypolicyviolation event", async () => {
    // THE CASE THE SECOND GUARD GOT WRONG, and the reason this one exists. The stubbed
    // `addEventListener` records nothing and no event is ever dispatched -- which is exactly
    // WebKit's behaviour in a worker: it refuses the request and fires nothing. The previous
    // guard concluded "no policy" and refused every operation in a browser that was
    // enforcing the policy correctly.
    const outcome = await runGuard("blob:", "policy-in-force");
    expect(outcome.policed).toBe(true);
  });

  it("refuses a worker created from a plain URL", async () => {
    // The browser should already have refused to build this, but a worker started from a
    // context this policy does not govern would still arrive here.
    const outcome = await runGuard("https:", "policy-in-force");
    expect(outcome.createdFromBlob).toBe(false);
    expect(outcome.policed).toBe(false);
  });

  it("refuses a blob: worker with NO policy in force", async () => {
    // THE CASE THE FIRST GUARD GOT WRONG. A Blob worker inherits the creating document's
    // policy -- whatever that is. A page that omitted the layout, or a host that mangled the
    // meta tag, yields a blob: worker with an empty policy.
    //
    // Here the probe 404s rather than being refused, and `fetch` resolves on a 404 -- so
    // "nothing refused it" is observable without any event.
    const outcome = await runGuard("blob:", "no-policy");
    expect(outcome.createdFromBlob).toBe(true);
    expect(outcome.policed, "an unrefused probe means there is no policy").toBe(false);
  });

  it("does not mistake a broken network for a policy", async () => {
    // Without the control, every request failing would look identical to a policy refusing
    // the probe -- and the guard would pass with nothing enforcing anything. The control is
    // an allowlisted fetch that must succeed first.
    const outcome = await runGuard("blob:", "network-broken");
    expect(outcome.policed).toBe(false);
  });

  it("gives up on a probe that never settles, and fails closed", async () => {
    // The wait must be BOUNDED. An unbounded one would hang the worker on its first message
    // rather than refuse it -- a worse failure than the one the guard prevents, and a silent
    // one.
    const started = Date.now();
    const outcome = await runGuard("blob:", "probe-hangs");
    const elapsed = Date.now() - started;

    expect(outcome.policed, "an inconclusive probe is not a policy").toBe(false);
    expect(elapsed, `the guard took ${elapsed}ms to give up`).toBeLessThan(8_000);
  });

  it("resolves as soon as the probe settles, without waiting out the timeout", async () => {
    // Bounded does not mean slow: a guard that always waited the full timeout would add that
    // to every worker's startup.
    const started = Date.now();
    const outcome = await runGuard("blob:", "policy-in-force");
    const elapsed = Date.now() - started;

    expect(outcome.policed).toBe(true);
    expect(elapsed, `the guard took ${elapsed}ms despite an immediate refusal`).toBeLessThan(200);
  });
});
