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
import { BUNDLE_RENDER_WORKER, BUNDLE_WORKER, ENGINE_ORIGIN } from "../generated/engines.js";
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
   * MEASURED FOR RENDERING TOO, SINCE #57, and it is why there is no second constant. The
   * slowest honest PAGE RENDER in the corpus is `objstm-bomb.pdf` at about 1.2 s -- roughly
   * 124x the next slowest fixture, and twice the 611 ms that set this number. A render budget
   * tighter than this one would refuse a document that is in the corpus on a device four times
   * slower than the runner, so the two workloads share a budget and that is now a measurement
   * rather than an inheritance. `e2e/measure.spec.ts` re-derives it on every run.
   *
   * WHAT IS NOT MEASURED, said rather than implied: no corpus fixture approaches the 512 MB
   * `maxInputBytes` ceiling, so a file near it is outside this sample. If one is slower than
   * 12 s the failure is a typed `LimitExceeded` the page explains -- `max_duration_ms` is
   * cooperative and checked at checkpoints (ADR 0007) -- not a wrong answer and not a freeze.
   * A visible refusal is the right direction to fail in; ADR 0015 records it as the residual.
   */
  maxDurationMs: 12_000,
  maxPages: 10_000,
  /**
   * 4 Mpx (2048 x 2048), and it is the first number in this project that was CHOSEN.
   *
   * `Limits::DEFAULT.max_pixels` is 256 Mpx and stays there. That is a CALLER ceiling -- the
   * outer bound on any raster the core will produce for anyone -- and it is far too loose to
   * be a device ceiling: the PDF maximum page, 14400 x 14400 points rendered 1:1, is 207 Mpx
   * and 791 MiB, and it PASSES the default. Enforcing `max_pixels` at its default would be
   * enforcing nothing that matters on the platform where it matters most.
   *
   * What 4 Mpx allows, which is the honest way to state a ceiling:
   *
   *   a thumbnail, 120x160 CSS px at DPR 2   0.077 Mpx     1/54th of it
   *   a full page at 150 dpi (A4)            2.17  Mpx     fits twice over
   *   a full page at 200 dpi (A4)            3.87  Mpx     fits
   *   a full page at 300 dpi (A4)            8.70  Mpx     REFUSED
   *   any 1:1 render of A0 or larger                       REFUSED
   *
   * Those are print resolutions and this is a screen. At the ceiling one bitmap is 16 MiB,
   * and ADR 0027 §2 is why there is never more than one: the worker is serialised and Rust
   * orchestrates the loop, so the engine heap holds a single raster whatever the strip's
   * length.
   *
   * CHOSEN, NOT MEASURED, and it was NOT measured on a phone. It was computed on a desktop
   * from page geometry and bytes per pixel; the only device evidence behind it is ADR 0015
   * §7's still-open observation that on iOS a memory spike kills the whole TAB rather than
   * the worker. ADR 0027's *"The three revision points"* names this constant, what would move
   * it, and what it would become -- and the same record says plainly that no number in it was
   * measured on a phone, so nobody can mistake these for measurements the way every other
   * number in this file is one.
   *
   * A person cannot reach this ceiling through the thumbnail strip: the caller states the box
   * and PDFium scales the page into it, so a 14400-point page at thumbnail size costs what a
   * thumbnail costs. It guards our own code and other callers of the core.
   */
  maxPixels: 4 * 1024 * 1024,
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

/**
 * Which worker bundle a host builds, and how many modules it will report on the way up.
 *
 * TWO BUNDLES SINCE ADR 0026, and this type is the whole of their difference as far as the
 * page is concerned. `DOCUMENTS` is what every tool page has always loaded — qpdf, the seven
 * document operations. `RENDER` is PDFium, fetched only when something needs a picture of a
 * page, so a person who merges two files downloads none of it.
 *
 * **Both are driven through the same `createWorkerHost`.** A second lifecycle written by
 * copying the first would start without the fixes this one accumulated — the memoised
 * promise, crash-counting rather than respawn-counting, the ack-based watchdog clock — each
 * of which was a measured failure. What differs between the two hosts is this object and
 * nothing else; see ADR 0026 for the full table, including the two policies that are
 * deliberately the same (the start-up bound) and the one that is deliberately independent
 * (the circuit breaker, so a document that kills the renderer cannot take merging offline).
 */
export interface WorkerBundle {
  /** Where the bundle's source text is, and what it must hash to. */
  worker: { url: string; integrity: string; bytes: number };
  /** How many `starting` messages its start-up may use. Generated, never written by hand. */
  modules: number;
}

/**
 * The base bundle: qpdf, and every operation that produces or reads a document.
 *
 * IMPORTED AS A NAMED EXPORT RATHER THAN LOOKED UP IN A MAP, which is what keeps the render
 * bundle's URL and digest out of an island that never renders. Vite tree-shakes named exports;
 * it cannot tree-shake a property off an object literal somebody indexed.
 */
export const DOCUMENTS: WorkerBundle = BUNDLE_WORKER;

/** The render bundle: PDFium. Built on first use, by a page that needs a page picture. */
export const RENDER: WorkerBundle = BUNDLE_RENDER_WORKER;

export function createToolHost(
  deps: ToolHostDeps = browserDeps(),
  bundle: WorkerBundle = DOCUMENTS,
): ToolHost {
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

    const entry = bundle.worker;
    const response = await deps.fetch(entry.url, { integrity: entry.integrity });
    if (!response.ok) throw new Error("worker fetch failed");
    const source = await response.text();

    const built = createWorkerHost({
      // PER BUNDLE, NOT A CONSTANT. Both bundles fetch two modules today; see
      // `EXPECTED_ENGINE_MODULES` for why that coincidence is exactly the reason this is
      // passed. The number comes from the generated manifest, which is also what the bundle
      // itself loops over, so the host's cap on trust and the worker's fetch list have one
      // source.
      expectedEngineModules: bundle.modules,
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
