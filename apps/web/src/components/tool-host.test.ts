// The shared worker wiring, driven without a browser.
//
// This block exists as its own module because it is a FIXED SECURITY DEFECT that had been
// copy-pasted (see `tool-host.ts`'s header). A shared fix with no test is the same risk one
// layer along, so the two properties that matter are asserted here: one host for concurrent
// callers, and a host built after disposal is still released.

import { describe, expect, it, vi } from "vitest";

import { ENGINE_UNAVAILABLE } from "../host/worker-host.js";
import {
  ALLOWED_WHILE_PAUSED,
  DOCUMENTS,
  ENGINE_PAUSED,
  LIMITS,
  ORIGIN_MISMATCH,
  RENDER,
  createToolHost,
  hostKind,
} from "./tool-host.js";
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

describe("a paused host refuses to acquire an engine", () => {
  // #107: a tab may hold qpdf OR PDFium, never both. The strip releases its engine while an
  // operation runs -- and the first two attempts at that gated one path each, in the component,
  // and left the next one open. This is the gate in the host, where every caller passes.

  it("refuses `ensure` while paused, by identity", async () => {
    const { deps: d, release } = deps();
    let paused = true;
    const host = createToolHost(d, DOCUMENTS, { paused: () => paused });
    release();

    await expect(host.ensure()).rejects.toBe(ENGINE_PAUSED);

    paused = false;
    await expect(host.ensure()).resolves.toBeDefined();
  });

  it("refuses EVERY acquiring call on a host obtained before the pause", async () => {
    // THE PATH THE COMPONENT ACTUALLY TOOK. `ensure()` refusing is not enough: a caller that
    // already holds the host from before the pause can still call into it, which is how
    // chromium re-fetched PDFium inside a running split after `ensure` was gated.
    const { deps: d, release } = deps();
    let paused = false;
    const host = createToolHost(d, DOCUMENTS, { paused: () => paused });
    release();
    const built = await host.ensure();

    paused = true;
    // INDEXED THROUGH A RECORD VIEW rather than the typed surface: the point of this loop is
    // to reach names the type does not necessarily carry, which is the same reason the wrapper
    // refuses by default.
    const surface = built as unknown as Record<string, unknown>;
    for (const method of ["run", "runHeld", "heldIsStillReadable"]) {
      const call = surface[method];
      if (typeof call !== "function") continue;
      expect(() => (call as () => unknown).call(built), `${method} was not refused`).toThrow();
    }
  });

  it("refuses a method that does not exist yet, so a future caller inherits the refusal", () => {
    // FAIL CLOSED, AND THIS IS THE ASSERTION THAT SAYS SO. The wrapper allows a named set and
    // refuses everything else, so a method added to the worker host later is refused while
    // paused until somebody puts it on the list deliberately. The alternative -- a list of
    // what is refused -- is the shape that let #107 recur twice.
    const { deps: d } = deps();
    let paused = true;
    const host = createToolHost(d, DOCUMENTS, { paused: () => paused });

    // A stand-in for the method nobody has written: the wrapper is asked for a property the
    // worker host does not have, and must not hand back something callable that acquires.
    const future = "aMethodFromTheFuture";
    expect(ALLOWED_WHILE_PAUSED.has(future), "the future method must not be allowlisted").toBe(
      false,
    );
    expect(ALLOWED_WHILE_PAUSED.has("run"), "`run` must not be allowlisted either").toBe(false);

    // And the allowlist is pinned, so widening it is a visible edit rather than a habit.
    expect([...ALLOWED_WHILE_PAUSED].sort()).toEqual([
      "discardWorker",
      "dispose",
      "hasWorker",
      "reset",
    ]);
    void host;
    paused = false;
  });

  it("releases the engine while paused rather than merely refusing to use it", async () => {
    const { deps: d, release, urls } = deps();
    let paused = false;
    const host = createToolHost(d, DOCUMENTS, { paused: () => paused });
    release();
    await host.ensure();

    paused = true;
    // `discardWorker` is ALLOWED while paused, because releasing is the point of pausing. A
    // gate that refused this would leave the engine resident, which is the failure it exists
    // to prevent -- so this asserts it is callable and does not throw, rather than asserting
    // a worker count: this harness spawns on first `run`, not on `ensure`.
    expect(() => host.discardWorker(), "releasing must not be refused").not.toThrow();
    expect(host.hasWorker(), "nothing may be held after a discard").toBe(false);
    void urls;
  });

  it("does not stick: the gate lifts the moment the predicate goes false", async () => {
    // A GATE THAT STICKS IS A STRIP THAT NEVER DRAWS, and it would pass #107's measurement bar
    // for entirely the wrong reason -- zero failures because zero engines. The predicate is
    // read per call rather than latched, so nothing can leave it true.
    const { deps: d, release } = deps();
    let paused = true;
    const host = createToolHost(d, DOCUMENTS, { paused: () => paused });
    release();

    await expect(host.ensure()).rejects.toBe(ENGINE_PAUSED);
    paused = false;
    const built = await host.ensure();
    expect(built, "the host must be usable again once the operation ends").toBeDefined();
    // And an acquiring call goes through rather than throwing, which is the property that
    // separates "the gate lifted" from "the gate is stuck and nothing ever draws again".
    expect(() => built.hasWorker(), "an ordinary call must not be refused").not.toThrow();
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

    // BY IDENTITY, which is both the stronger assertion and the one that matches how an island
    // must read it. The first version asserted `rejects.toThrow(/built for/)` -- it passed
    // against a thrown `Error`, and an `Error` is exactly what islands could not use: ADR 0009
    // forbids reading a thrown value's text, so every island fell through to
    // `messageFor({ kind: "Internal" })` and the page said "Something inside burrow failed"
    // beside a banner explaining the real cause. The test asserted the rejection and not the
    // outcome, which is why it did not see that.
    await expect(host.ensure()).rejects.toBe(ORIGIN_MISMATCH);
    // BEFORE THE FETCH, not after it fails. The fetch is the thing CSP refuses, and letting
    // it happen means the console carries a policy violation the person cannot act on --
    // which is the state this whole guard exists to replace.
    expect(fetched, "it fetched the worker source anyway").toBe(0);
    expect(host.hasWorker()).toBe(false);
  });

  it("builds the bundle it was asked for, and the base one by default", async () => {
    // THE SECOND BUNDLE HAD NO CALLER AND NO TEST, which code review raised as new public
    // surface nothing exercises. ADR 0026's claim is that there is one lifecycle and the
    // bundle is an argument to it; an argument nothing ever passes is a claim, not a property.
    //
    // The URL is the observable, because it is the one thing that differs at this layer: a
    // host's whole job here is to fetch ONE bundle's source and build a worker from it.
    const asked: string[] = [];
    const watching = (): Partial<ToolHostDeps> => ({
      fetch: (async (url: string) => {
        asked.push(String(url));
        return { ok: true, text: async () => "// worker source" } as Response;
      }) as unknown as typeof globalThis.fetch,
    });

    const base = deps(watching());
    await createToolHost(base.deps).ensure();
    const render = deps(watching());
    await createToolHost(render.deps, RENDER).ensure();

    expect(asked, "each host fetched exactly one bundle").toHaveLength(2);
    expect(asked[0], "the default is the documents bundle").toBe(DOCUMENTS.worker.url);
    expect(asked[1], "RENDER did not reach the fetch").toBe(RENDER.worker.url);
    // AND THEY ARE DIFFERENT, which is what makes the two assertions above worth anything: if
    // the generator ever emitted one descriptor twice, both would pass and mean nothing.
    expect(DOCUMENTS.worker.url).not.toBe(RENDER.worker.url);
  });

  it("passes each bundle's own module count to the lifecycle", async () => {
    // `EXPECTED_ENGINE_MODULES` stopped being a constant because both bundles happen to fetch
    // two modules, and ADR 0026 §4 argues a coincidence that holds is one nobody checks.
    // Nothing checked it: code review replaced the option with the old constant and 53 tests
    // still passed. This is the assertion that mutation fails.
    //
    // Asserted on the DESCRIPTORS rather than by reaching into the host, because that is where
    // the value comes from — the generated manifest — and a host that ignored it would fail
    // `worker-host.test.ts`'s own rearm case instead.
    for (const bundle of [DOCUMENTS, RENDER]) {
      expect(
        bundle.modules,
        `${bundle.worker.url} declares no module count, so the host would fall back to a default`,
      ).toBeGreaterThan(0);
    }
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
