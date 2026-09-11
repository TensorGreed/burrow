// The harness API, declared once.
//
// Two projects consume it: `harness-driver.js`, which implements it, and `e2e/harness.ts`,
// which drives it from Playwright. It was declared only on the Playwright side until M1 PR
// 4a-ii, which meant the implementation was unchecked against the shape its callers assumed —
// a rename on one side and a passing type-check on the other.

/** One operation's outcome, as it reaches the page. */
export interface Reply {
  ok: boolean;
  kind: string;
  /** Computed in Rust, not derived from `kind`. ADR 0009. */
  fatal: boolean;
  message: string;
  pages: number;
  limit: string;
  /** Strings, not numbers: these are `u64` and can exceed 2^53. */
  requested: string;
  allowed: string;
  /** The worker-lifecycle verdict, also computed in Rust. See `web/recycle.rs`. */
  recycle: boolean;
  pdfiumHeapBytes: string;
  qpdfHeapBytes: string;
}

/** What a CSP probe inside a worker observed. */
export interface ProbeResult {
  blocked: boolean;
  violations: string[];
  /** Set when the probe worker could not start — a different fact from "blocked". */
  failed?: boolean;
}

/** The ceilings one operation runs under. Mirrors `WebLimits` in `burrow-wasm`. */
export interface HarnessLimits {
  maxInputBytes: number;
  maxMemoryBytes: number;
  maxDurationMs: number;
  maxPages: number;
  maxPixels: number;
}

/**
 * What the next worker is built to do wrong.
 *
 * Every one of these is implemented by a prologue prepended to the (already integrity-checked)
 * worker source by the test-only driver — never by a hook in production code. See the header
 * of `harness-driver.js`.
 */
export interface HarnessArming {
  /** Capture everything the worker writes to its console, for the console-silence test. */
  captureConsole?: boolean;
  /** Make the worker log this deliberately — the control that proves the capture works. */
  logCanary?: string | null;
  /** Replace this `__burrow_*` bridge global with one that throws. */
  poison?: string | null;
  /** Block the worker thread synchronously for this long inside an engine call. */
  hangMs?: number;
}

export interface BurrowHarness {
  ready(): Promise<boolean>;
  run(
    op: "page_count" | "structure_check",
    bytes: number[],
    options?: {
      password?: number[];
      attemptRecovery?: boolean;
      limits?: Partial<HarnessLimits>;
    },
  ): Promise<Reply>;
  /** Keep one `File` in page scope, so an operation can run against the same object twice. */
  holdFile(bytes: number[]): void;
  runHeld(
    op: "page_count" | "structure_check",
    options?: { limits?: Partial<HarnessLimits> },
  ): Promise<Reply>;
  /** Whether the held file is still readable — i.e. was not detached by a transfer. */
  heldIsStillReadable(): Promise<boolean>;
  spawnCount(): number;
  hasWorker(): boolean;
  state(): "idle" | "initialising" | "busy" | "dead" | "respawning";
  breakerOpen(): boolean;
  reset(): void;
  arm(options?: HarnessArming): void;
  workerConsole(): string[];
  discardWorker(): void;
  fetchFromWorker(url: string): Promise<ProbeResult>;
  workerInheritsCsp(): Promise<boolean>;
}

declare global {
  interface Window {
    burrowHarness: BurrowHarness;
  }
}
