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
// An earlier version of the guard checked only `self.location.protocol === "blob:"` and was
// named `INHERITS_PAGE_CSP`. A Blob worker inherits the creating document's policy *whatever
// that is*, including none — so the name asserted something the check did not establish. The
// third case below is the one that failed under the old logic.

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
  fetchBehaviour: "refused-by-policy" | "network-error" | "succeeds",
): Promise<GuardOutcome> {
  const listeners: Array<() => void> = [];

  const scope: Record<string, unknown> = {
    location: { protocol },
    addEventListener: (name: string, handler: () => void) => {
      if (name === "securitypolicyviolation") listeners.push(handler);
    },
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
      // The prelude fetches the three engine modules as well as the probe. Those must
      // succeed, or their promises reject unobserved and the run is drowned in unhandled
      // rejections that have nothing to do with what is being tested.
      if (url.endsWith(".wasm")) {
        return { ok: true };
      }
      if (fetchBehaviour === "succeeds") {
        return { ok: true };
      }
      // A real CSP refusal dispatches the violation event *and* rejects the promise. A
      // network error only rejects. Modelling both is the whole point of this test.
      if (fetchBehaviour === "refused-by-policy") {
        for (const listener of listeners) listener();
      }
      throw new TypeError("Failed to fetch");
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
  it("accepts a blob: worker whose probe request the policy refuses", async () => {
    const outcome = await runGuard("blob:", "refused-by-policy");
    expect(outcome.createdFromBlob).toBe(true);
    expect(outcome.policed, "a refused probe is what proves a policy is in force").toBe(true);
  });

  it("refuses a worker created from a plain URL", async () => {
    // The browser should already have refused to build this, but a worker started from a
    // context this policy does not govern would still arrive here.
    const outcome = await runGuard("https:", "refused-by-policy");
    expect(outcome.createdFromBlob).toBe(false);
    expect(outcome.policed).toBe(false);
  });

  it("refuses a blob: worker with NO policy in force", async () => {
    // THE CASE THE OLD GUARD GOT WRONG. A Blob worker inherits the creating document's
    // policy — whatever that is. A page that omitted the layout, or a host that mangled the
    // meta tag, yields a blob: worker with an empty policy. The old check returned true here
    // and would have processed files with the browser-enforced half silently absent.
    const outcome = await runGuard("blob:", "succeeds");
    expect(outcome.createdFromBlob).toBe(true);
    expect(outcome.policed, "an unrefused probe means there is no policy").toBe(false);
  });

  it("does not mistake a network error for a policy", async () => {
    // A failed request is not evidence of a policy. Without the violation event, a probe
    // that merely failed — an offline browser, a host hiccup — would read as "policed" and
    // the guard would pass with nothing enforcing anything.
    const outcome = await runGuard("blob:", "network-error");
    expect(outcome.policed).toBe(false);
  });

  it("gives up waiting for the violation event, and fails closed when it does", async () => {
    // The wait must be BOUNDED. An unbounded one would hang the worker on its first message
    // rather than refuse it — a worse failure than the one the guard prevents, and a silent
    // one. This is the case where the event never comes at all.
    const started = Date.now();
    const outcome = await runGuard("blob:", "network-error");
    const elapsed = Date.now() - started;

    expect(outcome.policed, "no violation observed must mean not policed").toBe(false);
    expect(elapsed, `the guard took ${elapsed}ms to give up`).toBeLessThan(2_000);
  });

  it("resolves as soon as the violation arrives, without waiting out the timeout", async () => {
    // The complement: bounded does not mean slow. A guard that always waited the full
    // timeout would add that to every worker's startup.
    const started = Date.now();
    const outcome = await runGuard("blob:", "refused-by-policy");
    const elapsed = Date.now() - started;

    expect(outcome.policed).toBe(true);
    expect(elapsed, `the guard took ${elapsed}ms despite an immediate violation`).toBeLessThan(100);
  });
});
