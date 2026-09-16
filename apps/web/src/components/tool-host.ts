// The worker wiring every tool page shares, in one place.
//
// # Why this is shared when the rest of the two islands deliberately is not
//
// `merge-messages.ts` and `rotate-messages.ts` are separate on purpose: the sentences are the
// product, and a shared `messageFor` with a per-tool lookup table would put them behind an
// indirection. The markup, the controls and the state are per-tool for the same reason.
//
// This block is different, and the argument is not line count. **It is a fixed security
// defect.** `MergeTool.svelte` records it: a guard flag set after an `await` let two callers
// each build a host, each spawn a worker with its own engine instances, and share one
// `workerUrl` binding so one revoked the other's — leaving an orphan worker holding file bytes
// for the life of the page. Found by security review, fixed by memoising the promise rather
// than the result.
//
// Rotate's island then copied it. Code review made the point that decided this: the next fix
// to that wiring would be made in one of two copies, and the two had **already started to
// diverge in their reasoning** — rotate's comment argued the race window was narrower because
// it takes one file, which is true and is not a reason to hold a weaker version of the fix.
//
// So the wiring is here, with its own test, and what stays in the components is what differs:
// which operations they run, what they say about them, and what they show.

import { ENGINE_UNAVAILABLE, createWorkerHost } from "../host/worker-host.js";
import { ENGINES, ENGINE_ORIGIN } from "../generated/engines.js";
import { readOrigin } from "../origin-guard.js";

/**
 * The ceilings a tool page runs under.
 *
 * ONE COPY. Two islands declaring these separately is two numbers to keep in step with each
 * other and with the prose on two pages — and `size-budget.test.ts` compares each page's prose
 * against its island's constants, which is a comparison that gets cheaper the fewer constants
 * there are.
 *
 * These are the page's request, not an enforcement: the core applies every ceiling and the
 * page reports what it refuses (`apps/web/CLAUDE.md`). A page that enforced one itself could
 * disagree with the core, which is the disagreement that rule exists to make impossible.
 */
export const LIMITS = {
  maxInputBytes: 512 * 1024 * 1024,
  maxMemoryBytes: 1024 * 1024 * 1024,
  /**
   * 12 SECONDS, AND IT IS THE NUMBER THAT BOUNDS A HOSTILE FILE'S FREEZE.
   *
   * It was 120_000. `docs/security/exposure-2026-09-14-qpdf-uaf.md` accepted, as a residual,
   * that a crafted document freezes an operation for the watchdog budget before failing --
   * and recorded that budget as 60 s, which was already stale: the page asks for 120 s, and
   * `WATCHDOG_GRACE_MS` puts the real bound at 120.5. **Two minutes of frozen tab.**
   *
   * That was accepted about a LOCAL build. On a public URL it is a different question: a
   * minute-long freeze reads as a broken site, and most people close the tab well before the
   * error lands -- so the honest failure never reaches them and the site looks broken rather
   * than defended.
   *
   * MEASURED, NOT CHOSEN. `e2e/measure.spec.ts` walks the corpus and times every operation
   * that SUCCEEDS, because a budget under real work turns a working tool into one that
   * refuses honest documents -- which is worse than the freeze, since the freeze ends in a
   * correct answer and a premature refusal is a wrong one. What it found, in a browser:
   *
   *   ordinary documents (1-137 pages)          1-9 ms
   *   9,864 pages, just under `maxPages`        79-148 ms
   *   27,400 pages (past the ceiling, for slope) 215 ms
   *   `objstm-bomb.pdf`, structure-dense        611 ms   <- the slowest that succeeds
   *
   * The cost scales with STRUCTURE rather than with file size, which is why a document at the
   * page ceiling is cheaper than a 204 KB bomb. 12 s is ~20x the slowest honest operation
   * measured, which leaves an ordinary document three times the margin it needs on a machine
   * far slower than the runner.
   *
   * WHAT IS NOT MEASURED, said rather than implied: no corpus fixture approaches the 512 MB
   * `maxInputBytes` ceiling, so a file near it is outside this sample. If one is slower than
   * 12 s the failure is a typed `LimitExceeded` the page explains -- `max_duration_ms` is
   * cooperative and checked at checkpoints (ADR 0007) -- not a wrong answer and not a freeze.
   * A visible refusal is the right direction to fail in; ADR 0015 records it as the residual.
   */
  maxDurationMs: 12_000,
  maxPages: 10_000,
  maxPixels: 256 * 1024 * 1024,
} as const;

/**
 * A host verdict, as a `messageFor` expects it.
 *
 * `worker-host.js` produces two kinds that are not `burrow_types::Error` variants:
 * `ENGINE_UNAVAILABLE` when the breaker has latched, and `CANCELLED` when the page stopped a
 * request before it was posted. Normalised in one place so two operations on one page cannot
 * describe the same verdict differently.
 */
export function hostKind(kind: string): string {
  return kind === ENGINE_UNAVAILABLE ? "EngineUnavailable" : kind;
}

export interface ToolHost {
  /** The host, building it on first use. Never on page load. */
  ensure(): Promise<ReturnType<typeof createWorkerHost>>;
  /** Whether a live worker is held. Asked, never inferred from a reply. */
  hasWorker(): boolean;
  /** Terminate the worker in flight. The caller owns its own generation counter. */
  discardWorker(): void;
  /** Close the circuit breaker. Wired to a deliberate gesture. */
  reset(): void;
  /** Release everything, including a host still being built. */
  dispose(): void;
}

/** What `createToolHost` needs from the browser, injected so the test can drive it. */
export interface ToolHostDeps {
  fetch: typeof globalThis.fetch;
  createObjectURL: (blob: Blob) => string;
  revokeObjectURL: (url: string) => void;
  Worker: typeof globalThis.Worker;
  now: () => number;
  setTimer: (fn: () => void, ms: number) => unknown;
  clearTimer: (handle: unknown) => void;
  /**
   * The origin the page is being served from.
   *
   * INJECTED LIKE EVERY OTHER DEPENDENCY, so `tool-host.test.ts` can drive the mismatch
   * branch without a browser. Reading `location.origin` directly here would make the one
   * interesting case the one case no test could reach.
   */
  origin: string;
}

function browserDeps(): ToolHostDeps {
  return {
    fetch: globalThis.fetch.bind(globalThis),
    createObjectURL: (blob) => URL.createObjectURL(blob),
    revokeObjectURL: (url) => URL.revokeObjectURL(url),
    Worker: globalThis.Worker,
    now: () => performance.now(),
    setTimer: (fn, ms) => setTimeout(fn, ms),
    clearTimer: (handle) => clearTimeout(handle as number),
    origin: globalThis.location.origin,
  };
}

/**
 * Build the worker host a tool page runs on.
 *
 * The worker's SOURCE is fetched with its integrity digest and the worker is built from a
 * Blob. Not a convenience: a dedicated worker loaded from a same-origin script URL does not
 * inherit the page's CSP — it takes its policy from that script's response headers, and a
 * static host sends none (ADR 0014 §1a, measured). The worker is the only place file bytes
 * ever exist.
 */
/**
 * Thrown by `ensure()` when the page is not on the origin this build was made for.
 *
 * COMPARED BY IDENTITY, NEVER READ. ADR 0009 §2 is the reason: a thrown value can carry
 * module output, and module output can carry input-derived bytes, so an island that read a
 * thrown value's text would be a path for file content to reach the interface. A unique
 * object has no text to read and still tells the island exactly which failure it caught.
 *
 * It is not an `Error`. An `Error` invites `error.message` at the catch site, which is the
 * thing being prevented.
 */
export const ORIGIN_MISMATCH: unique symbol = Symbol("burrow.origin-mismatch");

/**
 * What an island shows when it catches {@link ORIGIN_MISMATCH}.
 *
 * SHARED, because all four islands owe the same sentence and the failure has nothing to do
 * with which tool is on the page. It points at the banner rather than repeating it: the
 * banner carries both origins and the rebuild, and saying it twice in different words is how
 * two explanations of one fact drift apart.
 *
 * `retryable: false` -- there is nothing on this page a person can do. The fix is a rebuild.
 */
export function originMismatchNotice(): { title: string; next: string; retryable: boolean } {
  return {
    title: "This copy of Not Only PDF was built for a different address.",
    next:
      "The tools cannot run here. Nothing is wrong with your file and nothing has been sent " +
      "anywhere. The notice at the top of this page has the detail.",
    retryable: false,
  };
}

export function createToolHost(deps: ToolHostDeps = browserDeps()): ToolHost {
  let host: ReturnType<typeof createWorkerHost> | null = null;
  let workerUrl: string | null = null;
  let building: Promise<ReturnType<typeof createWorkerHost>> | null = null;
  let disposed = false;

  async function build() {
    // THE ORIGIN, BEFORE THE FETCH. `ENGINES.worker.url` is absolute and on the origin this
    // build was made for (ADR 0014 §4), so on a wrongly-deployed copy this fetch is
    // cross-origin and CSP refuses it -- and what the person sees is "something inside burrow
    // failed", which is the interface blaming itself for a deployment mistake.
    //
    // `src/origin-guard.ts` already puts a banner at the top of every page saying the tools
    // will not work here. This is the other half: not attempting the work, so the explanation
    // on screen is not followed by a generic error that contradicts it. One check here covers
    // all four islands, because they all come through this factory.
    //
    // THE FIRST VERSION THREW AN `Error` AND THIS COMMENT WAS FALSE. Every island catches a
    // failed `ensure()` and renders `messageFor({ kind: "Internal" })` -- "Something inside
    // burrow failed" -- so the contradiction it claimed to have removed was still on screen,
    // beside the banner. Found by code review; the test asserted only that `ensure()`
    // rejected, which is why nothing saw it.
    //
    // A SENTINEL, NOT A MESSAGE STRING. ADR 0009 forbids an island reading a thrown value's
    // text, because a thrown value can carry module output and module output can carry
    // input-derived bytes. `ORIGIN_MISMATCH` is compared by identity, so the island learns
    // WHICH failure this is without reading anything out of it.
    const verdict = readOrigin({
      builtFor: ENGINE_ORIGIN,
      servedFrom: deps.origin,
    });
    if (verdict.kind === "mismatch") {
      throw ORIGIN_MISMATCH;
    }

    const entry = ENGINES.worker;
    const response = await deps.fetch(entry.url, { integrity: entry.integrity });
    if (!response.ok) throw new Error("worker fetch failed");
    const source = await response.text();

    const built = createWorkerHost({
      spawn: () => {
        if (workerUrl) deps.revokeObjectURL(workerUrl);
        workerUrl = deps.createObjectURL(new Blob([source], { type: "text/javascript" }));
        return new deps.Worker(workerUrl);
      },
      release: () => {
        if (workerUrl) {
          deps.revokeObjectURL(workerUrl);
          workerUrl = null;
        }
      },
      now: deps.now,
      setTimer: deps.setTimer,
      clearTimer: deps.clearTimer,
    });
    host = built;
    // Destroyed while the fetch was in flight: the host exists now and nothing holds it, so it
    // is released here rather than outliving the island that asked for it.
    if (disposed) built.dispose();
    return built;
  }

  return {
    // THE PROMISE IS MEMOISED, NOT THE RESULT. See this file's header: memoising the result
    // means a guard flag set after an `await`, and two callers inside that window each get a
    // worker while only one is reachable.
    ensure() {
      building ??= build();
      return building;
    },
    hasWorker: () => host?.hasWorker() ?? false,
    discardWorker: () => host?.discardWorker(),
    reset: () => host?.reset(),
    dispose() {
      disposed = true;
      // THROUGH THE PROMISE, not the binding: `host` is assigned only after two awaits, so a
      // page destroyed during them would otherwise never dispose and would leave a worker
      // holding the file blob alive. Found by security review.
      void building?.then((built) => built.dispose());
      host?.dispose();
    },
  };
}
