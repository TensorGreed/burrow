// The shared worker wiring, driven without a browser.
//
// This block exists as its own module because it is a FIXED SECURITY DEFECT that had been
// copy-pasted (see `tool-host.ts`'s header). A shared fix with no test is the same risk one
// layer along, so the two properties that matter are asserted here: one host for concurrent
// callers, and a host built after disposal is still released.

import { describe, expect, it, vi } from "vitest";

import { ENGINE_UNAVAILABLE } from "../host/worker-host.js";
import { LIMITS, createToolHost, hostKind } from "./tool-host.js";
import type { ToolHostDeps } from "./tool-host.js";
import { ENGINE_ORIGIN } from "../generated/engines.js";

/** A worker that does nothing, and records that it was made. */
class FakeWorker {
  static made = 0;
  static terminated = 0;
  constructor() {
    FakeWorker.made += 1;
  }
  addEventListener() {}
  removeEventListener() {}
  postMessage() {}
  terminate() {
    FakeWorker.terminated += 1;
  }
}

/** Dependencies that never touch the network or the DOM, with a controllable fetch. */
function deps(overrides: Partial<ToolHostDeps> = {}): {
  deps: ToolHostDeps;
  urls: { created: number; revoked: number };
  release: () => void;
} {
  const urls = { created: 0, revoked: 0 };
  let release = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });

  return {
    urls,
    release,
    deps: {
      fetch: (async () => {
        await gate;
        return { ok: true, text: async () => "// worker source" } as Response;
      }) as unknown as typeof globalThis.fetch,
      createObjectURL: () => {
        urls.created += 1;
        return `blob:fake-${urls.created}`;
      },
      revokeObjectURL: () => {
        urls.revoked += 1;
      },
      // THE BUILD'S OWN ORIGIN BY DEFAULT, so every existing case keeps testing what it was
      // testing. The mismatch branch is a case that overrides it, below -- if the default
      // here were a mismatch, every test in this file would be exercising the refusal.
      origin: ENGINE_ORIGIN,
      Worker: FakeWorker as unknown as typeof globalThis.Worker,
      now: () => 0,
      setTimer: (fn, ms) => setTimeout(fn, ms),
      clearTimer: (handle) => clearTimeout(handle as number),
      ...overrides,
    },
  };
}

describe("the shared tool host", () => {
  it("builds ONE host for callers that arrive together", async () => {
    // THE DEFECT THIS MODULE EXISTS FOR. A guard flag set after an `await` let two callers
    // inside the fetch window each build a host, each spawn a worker with its own engine
    // instances, and share one `workerUrl` binding so one revoked the other's -- an orphan
    // worker holding file bytes for the life of the page. Found by security review on
    // `/merge-pdf`, then copied to `/rotate-pdf` before this module existed.
    const { deps: d, release } = deps();
    const host = createToolHost(d);

    const first = host.ensure();
    const second = host.ensure();
    release();

    expect(await first, "two callers inside the fetch window got different hosts").toBe(
      await second,
    );
  });

  it("fetches the worker source once, however many callers there are", async () => {
    const { deps: d, release } = deps();
    const fetched = vi.fn(d.fetch);
    const host = createToolHost({ ...d, fetch: fetched as unknown as typeof globalThis.fetch });

    void host.ensure();
    void host.ensure();
    void host.ensure();
    release();
    await host.ensure();

    expect(fetched).toHaveBeenCalledTimes(1);
  });

  it("fetches the worker source with its integrity digest", async () => {
    // The whole reason the worker is built from a Blob rather than a URL: a worker loaded
    // from a script URL does not inherit the page's CSP (ADR 0014 §1a). Pinning the source is
    // what makes the Blob safe to build from.
    const { deps: d, release } = deps();
    const fetched = vi.fn(d.fetch);
    const host = createToolHost({ ...d, fetch: fetched as unknown as typeof globalThis.fetch });
    release();
    await host.ensure();

    const [, init] = fetched.mock.calls[0] as [string, RequestInit];
    expect(init.integrity, "the worker source was fetched without its digest").toMatch(/^sha/);
  });

  it("disposes a host that finished building after the page was destroyed", async () => {
    // `host` is assigned only after both awaits, so an island destroyed during the fetch used
    // to see nothing to dispose -- and the worker it went on to build outlived the component
    // holding the file blob. Found by security review.
    const { deps: d, release } = deps();
    const host = createToolHost(d);

    const building = host.ensure();
    host.dispose();
    release();
    const built = await building;

    // `dispose()` on the underlying host releases the worker; asking it is the observable
    // form of "it was disposed".
    expect(built.hasWorker(), "the late-built host kept a worker after disposal").toBe(false);
  });

  it("reports no worker before one is built, rather than throwing", () => {
    const { deps: d } = deps();
    const host = createToolHost(d);
    expect(host.hasWorker()).toBe(false);
    // These are wired to page controls that exist before any file is chosen. They must be
    // safe to call on a host nobody has built yet.
    expect(() => host.discardWorker()).not.toThrow();
    expect(() => host.reset()).not.toThrow();
    expect(() => host.dispose()).not.toThrow();
  });
});

describe("a host verdict, as a message kind", () => {
  it("maps the breaker's sentinel to the kind the message layer knows", () => {
    // IT IS THE IDENTITY TODAY, and that is worth saying rather than hiding behind a passing
    // test. `worker-host.js` exports `ENGINE_UNAVAILABLE = "EngineUnavailable"`, so this
    // function currently changes nothing -- I wrote this test expecting a different sentinel
    // and it failed, which is how I found out.
    //
    // It stays because the two names are not the same thing: one is a host sentinel and one
    // is a `burrow_types::Error` variant, and they are equal by coincidence rather than by
    // contract. The assertion is written against the exported sentinel rather than a literal,
    // so if the host ever renames it this fails here instead of in a tool page's messages.
    expect(hostKind(ENGINE_UNAVAILABLE)).toBe("EngineUnavailable");
  });

  it("passes every other kind through untouched", () => {
    for (const kind of ["Malformed", "LimitExceeded", "Internal", "PasswordRequired"]) {
      expect(hostKind(kind)).toBe(kind);
    }
  });
});

describe("the ceilings a tool page runs under", () => {
  it("are the ones both pages' prose states", () => {
    // `size-budget.test.ts` compares each page's prose against its island's constants. With
    // one shared `LIMITS` that is one comparison against N pages rather than N pairs -- this
    // pins the two numbers the prose quotes so a change here has to be deliberate.
    expect(LIMITS.maxInputBytes).toBe(512 * 1024 * 1024);
    expect(LIMITS.maxPages).toBe(10_000);
  });
  it("refuses to start the engines when the page is not on the origin it was built for", async () => {
    // THE OTHER HALF OF `src/origin-guard.ts`. The banner says the tools will not work here;
    // this is what stops them trying. Without it a person reads the banner, uses the tool
    // anyway, and gets "something inside burrow failed" -- the interface blaming itself for a
    // deployment mistake, and contradicting the explanation directly above it.
    let fetched = 0;
    const { deps: d } = deps({
      origin: "https://somewhere.else",
      fetch: (async () => {
        fetched += 1;
        return { ok: true, text: async () => "// worker source" } as Response;
      }) as unknown as typeof globalThis.fetch,
    });
    const host = createToolHost(d);

    await expect(host.ensure()).rejects.toThrow(/built for/);
    // BEFORE THE FETCH, not after it fails. The fetch is the thing CSP refuses, and letting
    // it happen means the console carries a policy violation the person cannot act on --
    // which is the state this whole guard exists to replace.
    expect(fetched, "it fetched the worker source anyway").toBe(0);
    expect(host.hasWorker()).toBe(false);
  });

  it("starts normally on the origin it WAS built for", async () => {
    // The control. Without it the assertion above would also pass against a host that refused
    // unconditionally, which would break every correctly-deployed page -- the worse failure.
    const { deps: d, release } = deps();
    const host = createToolHost(d);
    const pending = host.ensure();
    release();
    await expect(pending).resolves.toBeDefined();
  });
});
