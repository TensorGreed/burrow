// Every transition of the worker lifecycle, against a fake worker and a fake clock.
//
// This is the test half of ADR 0009's web contract. The contract cannot be enforced by any
// type system — a trap on `wasm32-unknown-unknown` leaves the worker alive and fires no event,
// so nothing in Rust can assert the page discarded the instance. ADR 0009 says so explicitly:
// "It needs a test — one that panics deliberately, asserts the page discards the instance, and
// asserts the next operation succeeds on a fresh one."
//
// The browser half of that lives in `e2e/recovery.spec.ts`, where a real trap in real
// WebAssembly is what produces the `fatal` reply. This half is where the *awkward* cases live,
// because they need timing a browser will not reproduce on demand: a result arriving after the
// watchdog fired, a crash during initialisation, a request made while a respawn is in flight.
//
// Nothing here waits on real time. `createFakeClock` is the page-side form of ADR 0007's
// injectable clock, for the same stated reason: a timeout test that really waits is a test
// nobody runs.

import { afterEach, describe, expect, test, vi } from "vitest";

import { createFakeClock, createFakeWorkerFactory, workerReply } from "./fake-worker.js";
import {
  CANCELLED,
  createWorkerHost,
  ENGINE_UNAVAILABLE,
  EXPECTED_ENGINE_MODULES,
  WATCHDOG_GRACE_MS,
} from "./worker-host.js";

type Factory = ReturnType<typeof createFakeWorkerFactory>;
type Clock = ReturnType<typeof createFakeClock>;
type Host = ReturnType<typeof createWorkerHost>;

// The workspace default is five minutes, for the two real Astro builds
// `production-build.test.ts` runs. Nothing here waits on anything, so a case that has not
// finished in a second is a hang — and a hang that takes five minutes to report is a hang
// nobody debugs.
vi.setConfig({ testTimeout: 2_000 });

const MAX_DURATION_MS = 1_000;
const INIT_TIMEOUT_MS = 5_000;
// DELIBERATELY DIFFERENT FROM `INIT_TIMEOUT_MS`. The two bounds shared one constant in the
// source until security review separated them, and a test that gave them the same value here
// could not tell which one had fired.
// Longer than the queue-time a case may legitimately simulate (`MAX_DURATION_MS * 3`) and
// different from `INIT_TIMEOUT_MS`, so a firing bound is identifiable.
const ACK_TIMEOUT_MS = 4_000;

/** Everything a case needs, wired together. */
function build(
  factoryOptions: Parameters<typeof createFakeWorkerFactory>[0] = {},
  /**
   * How many `starting` messages this host will honour.
   *
   * A PARAMETER, because ADR 0026 made it one: there are two worker bundles and the number is
   * a property of a bundle rather than of the host. Every existing case passes nothing and
   * gets the default, which is what keeps them testing what they were testing.
   */
  expectedEngineModules: number = EXPECTED_ENGINE_MODULES,
): {
  factory: Factory;
  clock: Clock;
  host: Host;
} {
  const factory = createFakeWorkerFactory(factoryOptions);
  const clock = createFakeClock();
  const host = createWorkerHost({
    spawn: factory.spawn,
    release: factory.release,
    now: clock.now,
    setTimer: clock.setTimer,
    clearTimer: clock.clearTimer,
    maxDurationMs: MAX_DURATION_MS,
    initTimeoutMs: INIT_TIMEOUT_MS,
    ackTimeoutMs: ACK_TIMEOUT_MS,
    breakerRespawns: 3,
    breakerWindowMs: 60_000,
    expectedEngineModules,
  });
  return { factory, clock, host };
}

/** Let queued microtasks run. The fake replies asynchronously, as a real worker does. */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

/** A request the host will post. The content is irrelevant to the lifecycle. */
const operation = { op: "page_count", blob: { size: 3 } };

let currentFactory: Factory | null = null;
let currentHost: Host | null = null;

afterEach(() => {
  // THE LEAK CHECK, on every case without exception.
  //
  // 4a-i's Rust fake hid a per-operation leak because it did not account for what it handed
  // out. The same blindness here would hide a worker that is never terminated — which is not
  // a tidiness problem: an abandoned worker keeps its compiled engines and its whole
  // linear memory for the life of the tab.
  currentHost?.dispose();
  currentFactory?.assertNoLeaks(0);
  expect(currentFactory?.releases(), "dispose() must release the factory's resource").toBe(1);
  currentFactory = null;
  currentHost = null;
});

/** Register a case's host and factory for the teardown assertions above. */
function track(built: { factory: Factory; clock: Clock; host: Host }) {
  currentFactory = built.factory;
  currentHost = built.host;
  return built;
}

// =====================================================================================
// The ordinary path
// =====================================================================================

describe("the ordinary path", () => {
  test("starts dead, initialises on the first request, and settles idle", async () => {
    const { host, factory } = track(build());
    expect(host.state()).toBe("dead");
    expect(host.spawnCount()).toBe(0);

    const ready = host.ready();
    expect(host.state(), "the FIRST spawn is initialising, not respawning").toBe("initialising");

    expect(await ready).toBe(true);
    expect(host.state()).toBe("idle");
    expect(host.spawnCount()).toBe(1);
    factory.assertNoLeaks(1);
  });

  test("an operation acks, runs, and returns its reply on the same worker", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    expect(host.state()).toBe("busy");

    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });
    worker.reply(workerReply(request.id, { pages: 10 }));

    const reply = await running;
    expect(reply.ok).toBe(true);
    expect(reply.pages).toBe(10);
    expect(host.state()).toBe("idle");
    expect(host.spawnCount(), "a successful operation costs no worker").toBe(1);
    expect(clock.outstanding(), "the watchdog timer must be cleared").toBe(0);
  });
});

// =====================================================================================
// ADR 0009: what costs a worker and what must not
// =====================================================================================

describe("recovery", () => {
  test.each([
    ["Malformed", "a damaged file"],
    ["LimitExceeded", "a file over a ceiling"],
    ["PasswordRequired", "an encrypted file"],
  ])("%s does not cost a worker", async (kind) => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });
    // `fatal` is computed in Rust. The host reads it and never re-derives it from `kind`,
    // which is what this asserts: a non-fatal reply of any kind keeps the instance.
    worker.reply(workerReply(request.id, { ok: false, kind, fatal: false }));

    const reply = await running;
    expect(reply.kind).toBe(kind);
    expect(host.spawnCount()).toBe(1);
    expect(host.state()).toBe("idle");
    expect(host.hasWorker()).toBe(true);
    factory.assertNoLeaks(1);
  });

  test("a fatal reply discards the instance and the next request gets a fresh one", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const poisoned = factory.latest();
    const request = poisoned.received.at(-1) as { id: number };
    poisoned.reply({ id: request.id, ack: true });
    poisoned.reply(workerReply(request.id, { ok: false, kind: "Internal", fatal: true }));

    const reply = await running;
    // The caller gets the REAL reply, not a substituted generic one: it carries the typed
    // outcome and is strictly more useful.
    expect(reply.kind).toBe("Internal");
    expect(host.state()).toBe("dead");
    expect(poisoned.terminations).toBe(1);

    // ADR 0009's required assertion: the next operation succeeds on a fresh worker.
    const next = host.run(operation);
    await settle();
    expect(host.spawnCount()).toBe(2);
    const fresh = factory.latest();
    expect(fresh).not.toBe(poisoned);
    const nextRequest = fresh.received.at(-1) as { id: number };
    fresh.reply({ id: nextRequest.id, ack: true });
    fresh.reply(workerReply(nextRequest.id, { pages: 3 }));
    expect((await next).pages).toBe(3);
  });

  test("nothing is ever retried automatically", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });
    worker.reply(workerReply(request.id, { ok: false, kind: "Internal", fatal: true }));
    await running;
    await settle();

    // A retry would appear as a second spawn with no second `run()`. ADR 0009: a retry
    // against a poisoned instance is worse than a visible failure.
    expect(host.spawnCount()).toBe(1);
    expect(host.hasWorker()).toBe(false);
  });

  test("the in-flight operation fails when its worker is discarded", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });
    worker.reply(workerReply(request.id, { ok: false, kind: "Internal", fatal: true }));

    expect((await running).kind).toBe("Internal");
    expect(host.state()).toBe("dead");
  });

  test("a QUEUED operation is not collateral damage — it runs on a fresh worker", async () => {
    // Operations are serialised because the worker is (see `queue` in worker-host.js), so at
    // most one is ever in flight. A request issued while another is running has not touched
    // the poisoned worker at all, and failing it would be punishing it for someone else's
    // file. It waits, and then runs on the replacement.
    const { host, factory } = track(build());
    await host.ready();

    const first = host.run(operation);
    const second = host.run(operation);
    await settle();

    const poisoned = factory.latest();
    expect(
      poisoned.received.filter((m) => typeof m === "object" && m !== null && !("type" in m)),
      "only ONE operation may be posted at a time",
    ).toHaveLength(1);

    const request = poisoned.received.at(-1) as { id: number };
    poisoned.reply({ id: request.id, ack: true });
    poisoned.reply(workerReply(request.id, { ok: false, kind: "Internal", fatal: true }));
    expect((await first).kind).toBe("Internal");

    await settle();
    expect(host.spawnCount(), "the queued request respawns rather than failing").toBe(2);
    const fresh = factory.latest();
    const next = fresh.received.at(-1) as { id: number };
    fresh.reply({ id: next.id, ack: true });
    fresh.reply(workerReply(next.id, { pages: 5 }));

    const reply = await second;
    expect(reply.ok, "a queued operation must not inherit the previous one's failure").toBe(true);
    expect(reply.pages).toBe(5);
  });

  test("a browser-killed worker fails its in-flight requests rather than hanging", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    // `onerror` with no reply: the browser killed it, and `send` resolves only on a reply, so
    // without this branch the promise would never settle at all.
    factory.latest().crash();

    const reply = await running;
    expect(reply.ok).toBe(false);
    expect(reply.kind).toBe("Internal");
    expect(host.state()).toBe("dead");
  });
});

// =====================================================================================
// The watchdog
// =====================================================================================

describe("the watchdog", () => {
  test("kills the worker at max_duration_ms plus grace and reports the limit", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });

    clock.advance(MAX_DURATION_MS + WATCHDOG_GRACE_MS - 1);
    expect(host.hasWorker(), "one millisecond early is not expired").toBe(true);

    clock.advance(1);
    const reply = await running;
    // The same shape Rust produces at a checkpoint. A user who hits this wall should not meet
    // two vocabularies depending on which side of the boundary noticed.
    expect(reply.kind).toBe("LimitExceeded");
    expect(reply.limit).toBe("max_duration_ms");
    // The host synthesises this string; `Stage::as_str` is where it comes from. Pinned here
    // because nothing else stops the JS literal drifting from the Rust one, and a differential
    // harness comparing stages would then see a divergence with no cause in either engine.
    expect(reply.stage).toBe("deadline");
    expect(reply.allowed).toBe(String(MAX_DURATION_MS));
    expect(worker.terminations).toBe(1);
    expect(host.state()).toBe("dead");
  });

  test("THE CLOCK STARTS AT THE ACK, not when the caller asked", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };

    // The worker sits on the request — queued behind engine start-up, or behind another
    // operation. Far longer than the whole duration budget.
    clock.advance(MAX_DURATION_MS * 3);
    expect(host.hasWorker(), "queue time must not expire the caller's deadline").toBe(true);

    worker.reply({ id: request.id, ack: true });
    clock.advance(MAX_DURATION_MS);
    expect(host.hasWorker(), "still inside the budget measured from the ack").toBe(true);

    clock.advance(WATCHDOG_GRACE_MS);
    expect((await running).limit).toBe("max_duration_ms");
  });

  test("the budget is per operation, because Limits are", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    // A caller with a more generous ceiling for this particular file. A watchdog fixed at
    // construction would kill it at the host's default instead, which is the wrong number.
    const running = host.run(operation, { maxDurationMs: MAX_DURATION_MS * 4 });
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });

    clock.advance(MAX_DURATION_MS + WATCHDOG_GRACE_MS + 1);
    expect(host.hasWorker(), "the host default must not override the caller's limit").toBe(true);

    clock.advance(MAX_DURATION_MS * 3);
    const reply = await running;
    expect(reply.limit).toBe("max_duration_ms");
    expect(reply.allowed, "the reply must name the limit the CALLER set").toBe(
      String(MAX_DURATION_MS * 4),
    );
  });

  test("a result arriving after the watchdog fired is dropped", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });
    clock.advance(MAX_DURATION_MS + WATCHDOG_GRACE_MS);

    const reply = await running;
    expect(reply.limit).toBe("max_duration_ms");

    // The engine call finally returns. The worker is terminated and replaced; this must
    // settle nothing, resurrect nothing, and throw nothing.
    //
    // Three things independently prevent it, and a mutation sweep found that disabling the
    // generation check alone changes nothing here — `discard()` nulls the handler and the
    // request is already out of `pending`. So this asserts the OUTCOME rather than any one
    // mechanism, which is the only honest thing to assert when the mechanisms overlap.
    expect(() => worker.reply(workerReply(request.id, { pages: 99 }))).not.toThrow();
    await settle();
    expect(reply.pages, "the caller keeps the answer it was already given").toBe(0);
    expect(host.state()).toBe("dead");
    expect(host.spawnCount(), "a late reply must not cause a spawn").toBe(1);
  });

  test("a worker that never acks is killed by the START-UP bound, not the duration one", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    clock.advance(MAX_DURATION_MS + WATCHDOG_GRACE_MS);
    expect(host.hasWorker(), "with no ack, the duration budget has not started").toBe(true);

    clock.advance(INIT_TIMEOUT_MS);
    const reply = await running;
    // NOT a duration limit. Nothing was blamed on the file: the worker never looked at it.
    expect(reply.kind).toBe("Internal");
    expect(reply.limit).toBe("");
    expect(factory.latest().terminations).toBe(1);
  });
});

// =====================================================================================
// Initialisation
// =====================================================================================

describe("initialisation", () => {
  test("a slow engine download is not a hang, however long it takes", async () => {
    // THE DEFECT THIS PINS, found by measurement rather than by review.
    //
    // `initTimeoutMs` bounded the WHOLE of start-up at 60 s, with a comment claiming that was
    // "comfortably above any measured cold load". Chrome's own "Slow 3G" profile refuted it:
    // the engines at 400 kbps were 140 seconds of network before anything was compiled. The
    // first file a person chose on a slow connection was refused at 60 s with "Something
    // inside burrow failed", the worker was discarded AS A CRASH, and three attempts would
    // have latched the breaker and taken the page offline. On a slow connection the engines
    // were never going to load at all.
    const { host, factory, clock } = track(
      build({
        onMessage: (instance, message) => {
          if (message?.type === "init") {
            // Every engine module landing slowly, the way `prelude.js` reports them: well
            // inside the stall bound each time, and far outside it in total.
            //
            // DERIVED FROM THE CONSTANT, not a literal. It was `3`, and spike 0004 took a
            // module out of the payload — so the third `starting` was ignored by the cap and
            // the clock ran past the bound, failing this test. That is the constant and its
            // test moving together, which is what `EXPECTED_ENGINE_MODULES`' own comment says
            // has to happen; deriving it means the next change moves only the constant.
            for (let i = 0; i < EXPECTED_ENGINE_MODULES; i += 1) {
              clock.advance(INIT_TIMEOUT_MS - 1_000);
              instance.reply({ starting: true });
            }
            clock.advance(INIT_TIMEOUT_MS - 1_000);
            instance.reply({ id: message.id, ready: true, ok: true });
          }
        },
      }),
    );

    expect(await host.ready(), "a transfer four times the stall bound was treated as a hang").toBe(
      true,
    );
    expect(host.hasWorker()).toBe(true);
    expect(host.breakerOpen(), "a slow network was counted as a crash").toBe(false);
    expect(factory.latest().terminations).toBe(0);
  });

  test("honours a bundle's OWN module count, not a constant", async () => {
    // ADR 0026 §4: `EXPECTED_ENGINE_MODULES` stopped being a constant because there are two
    // bundles and both happen to fetch two modules — "a coincidence that holds is one nobody
    // checks". Nothing checked it. Code review replaced `expectedEngineModules` at the call
    // site with the old constant and **53 tests still passed**, because no test ever passed a
    // different value. This is the case that mutation fails.
    //
    // THREE MODULES, spaced just inside the stall bound each time. Against the constant (2)
    // the third `starting` is ignored, the clock runs past the bound, and the worker is
    // declared dead — so a bundle that grew a module would have had its start-up bound
    // silently halved, on exactly the slow connections the bound exists for.
    const MODULES = 3;
    const { host, factory, clock } = track(
      build(
        {
          onMessage: (instance, message) => {
            if (message?.type === "init") {
              for (let i = 0; i < MODULES; i += 1) {
                clock.advance(INIT_TIMEOUT_MS - 1_000);
                instance.reply({ starting: true });
              }
              clock.advance(INIT_TIMEOUT_MS - 1_000);
              instance.reply({ id: message.id, ready: true, ok: true });
            }
          },
        },
        MODULES,
      ),
    );

    expect(
      await host.ready(),
      "a three-module bundle was declared dead, so the host used a number that is not its own",
    ).toBe(true);
    expect(host.breakerOpen(), "a slow three-module start-up was counted as a crash").toBe(false);
    expect(factory.latest().terminations).toBe(0);

    // AND THE CAP IS STILL A CAP at the value it was given — otherwise this case would pass
    // against a host that had simply stopped counting, which is the other way to make the
    // mutation invisible.
    expect(MODULES).not.toBe(EXPECTED_ENGINE_MODULES);
  });

  test("a worker cannot keep itself alive by reporting progress forever", async () => {
    // FOUND BY SECURITY REVIEW. With re-arms uncounted, a worker posting `starting` on a
    // timer keeps the bound alive indefinitely: `finish` never runs, the breaker never
    // latches, and the page shows "getting the engine ready" until somebody reloads -- the
    // exact hang the bound exists to close, reached through the mechanism that closes it.
    //
    // The cap is the number of engine modules, which is knowable, so a fourth message is a
    // bundle doing something this host does not model and is ignored rather than obeyed.
    const { host, factory, clock } = track(
      build({
        onMessage: (instance, message) => {
          if (message?.type === "init") {
            // Twice as many as there are modules, spaced to keep a naive timer alive forever.
            for (let i = 0; i < EXPECTED_ENGINE_MODULES * 2; i += 1) {
              clock.advance(INIT_TIMEOUT_MS - 1_000);
              instance.reply({ starting: true });
            }
          }
        },
      }),
    );

    // THE ASSERTION IS *WHEN* IT DIED, not that it did. The first version of this test sent
    // the messages and then advanced past the bound, and an uncapped host failed at that
    // final advance instead of during the loop -- so `ready()` was false either way and the
    // mutation passed all 37 tests. Caught by planting it, which is the only thing that
    // distinguishes a defence from a comment.
    //
    // With the cap, the message after the last permitted one cannot re-arm, so the bound
    // expires inside the loop and the host is already dead by the time it ends.
    const ready = host.ready();
    // The fake delivers messages on a microtask, so the loop above has not run yet.
    await settle();
    expect(
      host.state(),
      "the host survived more progress messages than there are engine modules, so a worker " +
        "can push its own bound out for as long as it likes",
    ).toBe("dead");
    expect(await ready).toBe(false);
    expect(factory.latest().terminations).toBe(1);
  });

  test("a worker that goes quiet is still declared dead", async () => {
    // The near-miss, and the reason the timer is a STALL timer rather than removed. Without
    // this, "a slow download survives" would be indistinguishable from "start-up is no longer
    // bounded at all", which is the hang the watchdog exists for.
    const { host, factory, clock } = track(
      build({
        onMessage: (instance, message) => {
          if (message?.type === "init") {
            // Two modules land, and then nothing ever again.
            instance.reply({ starting: true });
            clock.advance(1_000);
            instance.reply({ starting: true });
          }
        },
      }),
    );

    const ready = host.ready();
    clock.advance(INIT_TIMEOUT_MS + 1);
    expect(await ready, "a silent worker was waited on forever").toBe(false);
    expect(host.state()).toBe("dead");
    expect(factory.latest().terminations).toBe(1);
  });

  test("progress from a settled start-up does not push anything out", async () => {
    // A `starting` message carries no id, so nothing about it ties it to the initialisation
    // it came from. One arriving after start-up has finished -- or from a worker that has been
    // discarded -- must not re-arm a bound that is no longer running.
    const { host, factory, clock } = track(build());
    expect(await host.ready()).toBe(true);

    factory.latest().reply({ starting: true });
    clock.advance(INIT_TIMEOUT_MS * 10);

    expect(host.hasWorker(), "a stray progress message disturbed a healthy worker").toBe(true);
    expect(host.state()).toBe("idle");
  });

  test("a crash during initialisation leaves the host dead and fails the caller", async () => {
    const { host, factory } = track(
      build({
        onMessage: (instance, message) => {
          if (message?.type === "init") {
            // The worker dies mid-init — a wasm compile that traps, say. No reply.
            instance.crash();
          }
        },
      }),
    );

    expect(await host.ready()).toBe(false);
    expect(host.state()).toBe("dead");
    expect(host.hasWorker()).toBe(false);
    expect(factory.latest().terminations).toBe(1);
  });

  test("an init reply of ready:false discards the worker", async () => {
    const { host, factory } = track(
      build({
        onMessage: (instance, message) => {
          if (message?.type === "init") {
            instance.reply({ id: message.id, ready: false, fatal: true, kind: "Internal" });
          }
        },
      }),
    );

    expect(await host.ready()).toBe(false);
    expect(host.state()).toBe("dead");
    expect(factory.latest().terminations).toBe(1);
  });

  test("initialisation that never finishes is bounded by initTimeoutMs", async () => {
    const { host, factory, clock } = track(build({ onMessage: () => {} }));

    const ready = host.ready();
    await settle();
    expect(host.state()).toBe("initialising");

    clock.advance(INIT_TIMEOUT_MS);
    expect(await ready).toBe(false);
    expect(host.state()).toBe("dead");
    expect(factory.latest().terminations).toBe(1);
  });
});

// =====================================================================================
// Respawning
// =====================================================================================

describe("respawning", () => {
  test("a request made while a respawn is in progress joins it and spawns once", async () => {
    // Init is held open, so the respawn is genuinely in flight when the other callers land.
    const held: { resolve: () => void }[] = [];
    const { host, factory } = track(
      build({
        onMessage: (instance, message) => {
          if (message?.type === "init") {
            held.push({ resolve: () => instance.reply({ id: message.id, ready: true }) });
          }
        },
      }),
    );

    const first = host.ready();
    await settle();
    held[0].resolve();
    await first;

    // Kill it, so what follows is a respawn rather than a first spawn.
    const crashed = host.run(operation);
    await settle();
    const dying = factory.latest();
    const dyingRequest = dying.received.at(-1) as { id: number };
    dying.reply({ id: dyingRequest.id, ack: true });
    dying.reply(workerReply(dyingRequest.id, { ok: false, kind: "Internal", fatal: true }));
    await crashed;

    // An operation and a bare `ready()` arriving during the same respawn. `ready()` is NOT
    // queued — a page may ask for the engines while work is pending — so this is the case
    // where two callers genuinely race one transition.
    const a = host.run(operation);
    await settle();
    expect(host.state(), "a REPLACEMENT worker is respawning, not initialising").toBe("respawning");
    const b = host.ready();
    await settle();

    expect(host.spawnCount(), "one respawn, not two").toBe(2);

    held[1].resolve();
    await settle();
    expect(await b).toBe(true);

    const fresh = factory.latest();
    const request = fresh.received.at(-1) as { id: number };
    fresh.reply({ id: request.id, ack: true });
    fresh.reply(workerReply(request.id, { pages: 7 }));

    expect((await a).pages).toBe(7);
    expect(host.spawnCount()).toBe(2);
  });
});

// =====================================================================================
// Serialisation, and the ack
// =====================================================================================

describe("serialisation", () => {
  test("a cancel abandons what is queued, instead of spawning a worker to finish it", async () => {
    // THE BUG THIS PINS, found by security review.
    //
    // `discard()` settles the requests that are IN FLIGHT. A queued one has not been posted,
    // so there is nothing to settle -- and it used to go on and run: `ensureWorker()` found
    // the state `dead`, the breaker permitted a spawn (a page-initiated discard is not a
    // crash, deliberately), and a fresh worker compiled the engines to finish an operation the
    // person had already stopped. The page had returned to idle and said "Stopped."
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(operation, { maxDurationMs: INIT_TIMEOUT_MS * 5 });
    const queued = host.run(operation, { maxDurationMs: INIT_TIMEOUT_MS * 5 });
    await settle();
    expect(factory.instances().length, "one worker before the cancel").toBe(1);

    host.discardWorker();
    await settle();

    // BOTH answer, and neither is fatal in a way that would cost another worker.
    expect((await running).ok).toBe(false);
    const abandoned = await queued;
    expect(abandoned.ok).toBe(false);
    expect(abandoned.kind, "a cancelled request is not an engine failure").toBe(CANCELLED);
    expect(abandoned.fatal, "nothing ran, so nothing was poisoned").toBe(false);

    // THE MEASUREMENT. Without the counter this is 2: the queued request spawned its own.
    expect(
      factory.instances().length,
      "a cancelled request spawned a worker to finish work nobody was waiting for",
    ).toBe(1);
    expect(host.breakerOpen(), "a cancel is not a crash").toBe(false);
  });

  test("a cancel abandons only what was already queued, not what comes after", async () => {
    // The near-miss. A latch instead of a counter would refuse every later operation too, and
    // the test above would pass identically -- so the page would be dead after one Stop.
    const { host, factory } = track(build());
    await host.ready();

    const queued = host.run(operation, { maxDurationMs: INIT_TIMEOUT_MS * 5 });
    host.discardWorker();
    expect((await queued).kind).toBe(CANCELLED);

    const after = host.run(operation, { maxDurationMs: INIT_TIMEOUT_MS * 5 });
    await settle();
    const worker = factory.latest();
    const posted = worker.received.filter(
      (m) => typeof m === "object" && m !== null && !("type" in m),
    );
    expect(posted, "the operation after a cancel must actually be posted").toHaveLength(1);
    const id = (posted.at(-1) as { id: number }).id;
    worker.reply({ id, ack: true });
    worker.reply(workerReply(id, { pages: 7 }));
    expect((await after).pages).toBe(7);
  });

  test("an operation that is never acked is bounded by the ACK bound, not the start-up one", async () => {
    // FOUND BY SECURITY REVIEW. This window opens after `ensureWorker()` has resolved, so the
    // engines are already compiled and nothing in it depends on anybody's network -- but it
    // shared `initTimeoutMs`, which ADR 0018 raised 60 s -> 240 s for a 5.3 MB download. A
    // worker that died silently after init would have left a person watching a spinner four
    // times as long, with the crash counted four times as late.
    const { host, factory, clock } = track(
      build({
        onMessage: (instance, message) => {
          // Init answers; an operation never does.
          if (message?.type === "init") {
            instance.reply({ id: message.id, ready: true, ok: true });
          }
        },
      }),
    );
    await host.ready();

    const pending = host.run(operation, { maxDurationMs: MAX_DURATION_MS });
    // The post happens on the queue's microtask, so the timer does not exist until it has run.
    await settle();
    clock.advance(ACK_TIMEOUT_MS + 1);

    const reply = await pending;
    expect(reply.ok, "an unacked operation was never bounded").toBe(false);
    expect(reply.kind).toBe("Internal");
    expect(
      factory.latest().terminations,
      "the worker should have been discarded at the ack bound",
    ).toBe(1);
  });

  test("a queued operation cannot kill the one that is running", async () => {
    // THE BUG THIS PINS, found by security review and reproduced exactly here.
    //
    // The pre-ack timer bounds START-UP at 60 s and its expiry counts as a crash. The worker's
    // message loop is single-threaded, so a second concurrent operation could not be acked
    // until the first finished — and ITS pre-ack timer then fired on a healthy worker
    // mid-operation, killing the first caller (whose own budget had minutes left) with
    // `Internal`, and counting a crash. Three of those latch the breaker and the page stops
    // working at all.
    //
    // Serialising removes the timer from the waiting request entirely: a queued operation is
    // not posted, so it has no deadline, because nothing about waiting is the file's fault.
    const LONG = INIT_TIMEOUT_MS * 5;
    const { host, factory, clock } = track(build());
    await host.ready();

    const a = host.run(operation, { maxDurationMs: LONG });
    const b = host.run(operation, { maxDurationMs: LONG });
    await settle();

    const worker = factory.latest();
    const posted = () =>
      worker.received.filter((m) => typeof m === "object" && m !== null && !("type" in m));
    expect(posted(), "only ONE operation may be posted at a time").toHaveLength(1);

    const first = posted().at(-1) as { id: number };
    worker.reply({ id: first.id, ack: true });

    // Well past the start-up bound, and well inside A's own budget.
    clock.advance(INIT_TIMEOUT_MS * 2);
    expect(host.hasWorker(), "a waiting request must not kill the running one").toBe(true);
    expect(host.breakerOpen(), "and must not count as a crash").toBe(false);

    worker.reply(workerReply(first.id, { pages: 1 }));
    expect((await a).pages).toBe(1);

    await settle();
    const second = posted().at(-1) as { id: number };
    worker.reply({ id: second.id, ack: true });
    worker.reply(workerReply(second.id, { pages: 2 }));

    expect((await b).pages).toBe(2);
    expect(host.spawnCount(), "two operations, one worker").toBe(1);
  });

  test("operations complete in the order they were asked for", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const all = [host.run(operation), host.run(operation), host.run(operation)];
    const worker = factory.latest();
    const posted = () =>
      worker.received.filter((m) => typeof m === "object" && m !== null && !("type" in m));

    for (let i = 0; i < 3; i += 1) {
      await settle();
      expect(posted(), `only one in flight at step ${i}`).toHaveLength(i + 1);
      const request = posted().at(-1) as { id: number };
      worker.reply({ id: request.id, ack: true });
      worker.reply(workerReply(request.id, { pages: i + 1 }));
    }

    expect((await Promise.all(all)).map((r) => r.pages)).toEqual([1, 2, 3]);
  });

  test("a worker cannot extend its own deadline by acking twice", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };

    // Ten acks over ten budgets. A host that re-armed on each would never fire, and the
    // operation deadline would simply cease to exist.
    for (let i = 0; i < 10; i += 1) {
      worker.reply({ id: request.id, ack: true });
      clock.advance(MAX_DURATION_MS);
    }

    const reply = await running;
    expect(reply.kind).toBe("LimitExceeded");
    expect(reply.limit).toBe("max_duration_ms");
    expect(worker.terminations).toBe(1);
  });

  test("an ack on the init request cannot turn a start-up failure into a limit failure", async () => {
    // ADR 0015 §2: "On `initTimeoutMs`: fail with `Internal`. NEVER a duration limit." A
    // worker that acked its init instead of answering it would otherwise swap the start-up
    // bound for the operation budget and report a slow start as the file's fault.
    const { host, factory, clock } = track(
      build({
        onMessage: (instance, message) => {
          if (message?.type === "init") {
            instance.reply({ id: message.id, ack: true });
          }
        },
      }),
    );

    const ready = host.ready();
    await settle();

    clock.advance(MAX_DURATION_MS + WATCHDOG_GRACE_MS + 1);
    expect(host.hasWorker(), "an init must still be governed by the start-up bound").toBe(true);

    clock.advance(INIT_TIMEOUT_MS);
    expect(await ready).toBe(false);
    expect(host.state()).toBe("dead");
    expect(factory.latest().terminations).toBe(1);
  });
});

// =====================================================================================
// The circuit breaker
// =====================================================================================

describe("the circuit breaker", () => {
  /** Crash the current worker once. */
  async function crashOnce(host: Host, factory: Factory) {
    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number } | undefined;
    if (request) {
      worker.reply({ id: request.id, ack: true });
      worker.reply(workerReply(request.id, { ok: false, kind: "Internal", fatal: true }));
    }
    return running;
  }

  test("opens after three crashes in the window and refuses to spawn again", async () => {
    const { host, factory } = track(build());
    await host.ready();

    await crashOnce(host, factory);
    await crashOnce(host, factory);
    await crashOnce(host, factory);
    expect(host.spawnCount(), "each crash so far was replaced").toBe(3);

    const refused = await host.run(operation);
    expect(refused.kind).toBe(ENGINE_UNAVAILABLE);
    expect(refused.ok).toBe(false);
    expect(host.breakerOpen()).toBe(true);
    expect(host.spawnCount(), "an open breaker must spawn nothing at all").toBe(3);
  });

  test("RECYCLING IS NOT A CRASH and must never trip the breaker", async () => {
    // The bug this pins was real and was found by measurement, not review:
    // `e2e/measure.spec.ts` recorded two of five respawn samples as `0ms`, because counting
    // respawns rather than crashes meant three recycles opened the breaker. A page working
    // through a run of large files would have taken itself offline with `EngineUnavailable`
    // while every worker it had was perfectly healthy.
    const { host, factory } = track(build());
    await host.ready();

    for (let i = 0; i < 6; i += 1) {
      const running = host.run(operation);
      await settle();
      const worker = factory.latest();
      const request = worker.received.at(-1) as { id: number };
      worker.reply({ id: request.id, ack: true });
      worker.reply(workerReply(request.id, { recycle: true }));
      expect((await running).ok, `recycle ${i}`).toBe(true);
    }

    expect(host.breakerOpen(), "six recycles are six healthy operations").toBe(false);
    expect(host.spawnCount()).toBe(6);
  });

  test("a page-initiated discard is not a crash either", async () => {
    // What a "cancel" button does. A user cancelling three times in a minute must not be
    // told the engines are unavailable.
    const { host } = track(build());
    await host.ready();
    for (let i = 0; i < 5; i += 1) {
      host.discardWorker();
      expect(await host.ready(), `cancel ${i}`).toBe(true);
    }
    expect(host.breakerOpen()).toBe(false);
  });

  test("a crash outside the window does not count towards the breaker", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    await crashOnce(host, factory);
    await crashOnce(host, factory);
    // Long enough that the earlier crashes fall out of the sliding window.
    clock.advance(60_001);
    await crashOnce(host, factory);
    await crashOnce(host, factory);

    expect(host.breakerOpen(), "an occasional crash over a long session must not trip it").toBe(
      false,
    );
    const next = host.run(operation);
    await settle();
    expect(host.spawnCount()).toBe(5);
    factory.latest().crash();
    await next;
  });

  test("reset() closes it, and nothing else does", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();
    for (let i = 0; i < 3; i += 1) {
      await crashOnce(host, factory);
    }
    expect((await host.run(operation)).kind).toBe(ENGINE_UNAVAILABLE);

    // Time alone must not reopen it. "Stop respawning" has to mean stopped, or a page
    // retrying on a timer resumes the crash loop at a slower rate rather than ending it.
    // Note this is time enough for the window to empty, which is precisely the point: the
    // breaker latches, it does not merely rate-limit.
    clock.advance(60_000 * 10);
    expect((await host.run(operation)).kind).toBe(ENGINE_UNAVAILABLE);
    expect(host.spawnCount()).toBe(3);

    host.reset();
    const next = host.run(operation);
    await settle();
    expect(host.breakerOpen()).toBe(false);
    expect(host.spawnCount()).toBe(4);
    factory.latest().crash();
    await next;
  });
});

// =====================================================================================
// Heap-growth recycling
// =====================================================================================

describe("recycling", () => {
  test("a recycled reply reaches the caller intact, and the worker goes", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });
    // `recycle` is computed in Rust from `max_memory_bytes`; the host reads it. The heap
    // number comes from the fake, not from the assertion, so this cannot pass vacuously.
    worker.reply(
      workerReply(request.id, {
        pages: 4,
        recycle: true,
        engineHeapBytes: String(600 * 1024 * 1024),
      }),
    );

    const reply = await running;
    // RECYCLING IS NOT A FAILURE, and this is the assertion that says so: the caller's
    // result arrives intact, not substituted for the `Internal` a discard would otherwise
    // produce. The realistic bug is someone folding `recycle` into the `fatal` branch.
    //
    // The *statement order* in `handleMessage` — settle, then discard — is not what carries
    // this, and claiming it did would be a test that cannot fail: the entry is removed from
    // `pending` before either branch runs, so a discard can no longer reach it either way.
    // The order stands as the clearer expression of intent, and this asserts the outcome.
    expect(reply.ok).toBe(true);
    expect(reply.pages).toBe(4);
    expect(worker.terminations, "a grown worker must not be kept").toBe(1);
    expect(host.state()).toBe("dead");
    expect(host.spawnCount()).toBe(1);

    const next = host.run(operation);
    await settle();
    expect(host.spawnCount(), "the next operation gets a fresh worker").toBe(2);
    const fresh = factory.latest();
    const nextRequest = fresh.received.at(-1) as { id: number };
    fresh.reply({ id: nextRequest.id, ack: true });
    fresh.reply(workerReply(nextRequest.id));
    await next;
  });

  test("a reply without the flag keeps the worker", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });
    worker.reply(workerReply(request.id, { recycle: false }));

    await running;
    expect(host.spawnCount()).toBe(1);
    expect(host.hasWorker()).toBe(true);
    factory.assertNoLeaks(1);
  });
});

// =====================================================================================
// ADR 0023: an operation that delivers more than one document
// =====================================================================================
//
// The protocol is general — `{ part: { index, of }, output }` messages followed by an ordinary
// terminal reply — so redaction and any later multi-output operation inherit it rather than a
// split-specific field. What is asserted here is the host's half of it, which is the half no
// browser test can reach on demand: a dropped part message, a part carrying no bytes, and a
// watchdog that has to survive fifty units of work without firing on the honest case.

/** A split request. The host does not interpret it; the worker's reply shape is what matters. */
const splitOperation = { op: "split", blob: { size: 3 }, at: [1] };

/** Post `count` parts, then the terminal success. Returns what the caller was handed. */
function deliver(
  worker: { reply: (message: unknown) => void },
  id: number,
  count: number,
  options: { skip?: number; withoutOutput?: number } = {},
) {
  for (let index = 0; index < count; index += 1) {
    if (options.skip === index) continue;
    worker.reply({
      id,
      part: { index, of: count },
      output: options.withoutOutput === index ? undefined : new Blob([`part ${index}`]),
    });
  }
}

describe("multi-output delivery", () => {
  test("parts arrive one at a time and are handed over on the terminal reply", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    const seen: Array<{ part: number; of: number }> = [];
    const running = host.run(splitOperation, { onProgress: (p) => seen.push(p) });
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });

    // PROGRESS PER PART, not one jump at the end. The worker posts it between parts, and a
    // host that dropped it would leave a fifty-way split looking hung for its whole duration.
    worker.reply({ id: request.id, progress: { part: 1, of: 3 } });
    deliver(worker, request.id, 3);
    worker.reply({ id: request.id, progress: { part: 3, of: 3 } });
    worker.reply(workerReply(request.id, { pages: 3 }));

    const reply = await running;
    expect(reply.ok).toBe(true);
    expect(reply.parts?.length).toBe(3);
    expect(await reply.parts?.[1]?.text()).toBe("part 1");
    expect(seen).toEqual([
      { part: 1, of: 3 },
      { part: 3, of: 3 },
    ]);
    expect(host.state()).toBe("idle");
    expect(host.spawnCount(), "a successful split costs no worker").toBe(1);
    expect(clock.outstanding(), "the per-part watchdog must be cleared").toBe(0);
  });

  test("a part that never arrived is a failure, not a success with a hole in it", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(splitOperation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });

    // NINE OF TEN, with the LAST one present so `entry.parts.length` reaches the total. This
    // is the exact shape that passed the gate when it was written with `Array.prototype.every`,
    // which skips holes: the caller was handed ten parts, nine of them `undefined`.
    deliver(worker, request.id, 10, { skip: 4 });
    worker.reply(workerReply(request.id, { pages: 10 }));

    const reply = await running;
    expect(reply.ok).toBe(false);
    expect(reply.kind).toBe("Internal");
    expect(reply.parts, "a failed split hands over nothing").toBeUndefined();
    // AND THE WORKER GOES. It is mid-protocol with a session this host has forgotten; leaving
    // it would wedge the host in `busy` for the life of the tab.
    expect(worker.terminations).toBe(1);
    expect(host.state()).toBe("dead");
  });

  test("a part carrying no document is refused as soon as it arrives", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(splitOperation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });
    deliver(worker, request.id, 3, { withoutOutput: 1 });

    // NO TERMINAL REPLY IS SENT. The failure is named at the part, not several messages later
    // as "a part did not arrive".
    const reply = await running;
    expect(reply.ok).toBe(false);
    expect(reply.kind).toBe("Internal");
    expect(worker.terminations).toBe(1);
  });

  test("a split that fails partway delivers nothing, matching merge's rule", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(splitOperation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });

    // TWO OF TEN ARRIVED, and then the operation failed. ADR 0023 §3: `split` is defined as a
    // partition, and a subset of the parts is not a partition of anything -- so the two that
    // succeeded are dropped rather than handed over with the failure named beside them.
    deliver(worker, request.id, 2);
    worker.reply(workerReply(request.id, { ok: false, kind: "Malformed", fatal: false }));

    const reply = await running;
    expect(reply.ok).toBe(false);
    expect(reply.kind).toBe("Malformed");
    expect(reply.parts).toBeUndefined();
    // AND A MALFORMED FILE STILL COSTS NO WORKER, part-way through a split or not.
    expect(host.spawnCount()).toBe(1);
    expect(host.hasWorker()).toBe(true);
    factory.assertNoLeaks(1);
  });

  test("the watchdog is re-armed per part, so a long split is not one budget", async () => {
    const { host, factory, clock } = track(build());
    await host.ready();

    const running = host.run(splitOperation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });

    // TEN PARTS, each arriving just inside the budget, totalling ten times it. One budget for
    // the whole operation would have fired on part two -- and this is the honest case.
    for (let index = 0; index < 10; index += 1) {
      clock.advance(MAX_DURATION_MS + WATCHDOG_GRACE_MS - 1);
      worker.reply({ id: request.id, part: { index, of: 10 }, output: new Blob([`p${index}`]) });
    }
    worker.reply(workerReply(request.id, { pages: 10 }));

    const reply = await running;
    expect(reply.ok, "the watchdog fired on a split that was making progress").toBe(true);
    expect(reply.parts?.length).toBe(10);
    expect(host.spawnCount()).toBe(1);
  });

  test("a single-output operation is handed no parts field at all", async () => {
    const { host, factory } = track(build());
    await host.ready();

    const running = host.run(operation);
    await settle();
    const worker = factory.latest();
    const request = worker.received.at(-1) as { id: number };
    worker.reply({ id: request.id, ack: true });
    worker.reply(workerReply(request.id, { pages: 10 }));

    const reply = await running;
    // THE PROTOCOL IS OPT-IN. `partsExpected` is set by the first part message, so an
    // operation that posts none is untouched by any of the code above -- which is what makes
    // this shape general rather than a split-shaped branch every other operation walks past.
    expect(reply.ok).toBe(true);
    expect("parts" in reply).toBe(false);
  });
});
