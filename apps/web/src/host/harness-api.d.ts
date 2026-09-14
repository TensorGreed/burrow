// The harness API, declared once.
//
// Two projects consume it: `harness-driver.js`, which implements it, and `e2e/harness.ts`,
// which drives it from Playwright. It was declared only on the Playwright side until M1 PR
// 4a-ii, which meant the implementation was unchecked against the shape its callers assumed —
// a rename on one side and a passing type-check on the other.

/** One operation's outcome, as it reaches the page. */
export interface Reply {
  /** Bytes of the document an operation produced, or 0. The harness does not return it. */
  outputBytes?: number;
  ok: boolean;
  kind: string;
  /** Computed in Rust, not derived from `kind`. ADR 0009. */
  fatal: boolean;
  message: string;
  pages: number;
  limit: string;
  /** Which check fired: `prescan`, `size_estimate`, `measured`, … See `burrow_types::Stage`. */
  stage: string;
  /** Strings, not numbers: these are `u64` and can exceed 2^53. */
  requested: string;
  allowed: string;
  /** The worker-lifecycle verdict, also computed in Rust. See `web/recycle.rs`. */
  recycle: boolean;
  pdfiumHeapBytes: string;
  qpdfHeapBytes: string;
  /**
   * Every page's effective rotation, in page order. `page_rotations` only.
   *
   * Numbers rather than strings, unlike `requested` and `allowed`: a rotation is 0, 90, 180
   * or 270, so there is no `u64` here to lose precision on.
   */
  rotations?: number[];
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
  /**
   * Run one operation over base64-encoded bytes, with the password as a plain string.
   *
   * What the conformance corpus uses: a third the payload of a `number[]`, and the password
   * arrives in the form `expectations.json` records it.
   */
  runBase64(
    op: "page_count" | "structure_check" | "merge" | "rotate" | "reorder" | "page_rotations",
    base64: string,
    options?: {
      password?: string | null;
      attemptRecovery?: boolean;
      limits?: Partial<HarnessLimits>;
      /**
       * Further documents, base64, in order. `merge` only.
       *
       * Separate from `base64` rather than replacing it with a list, so the shape a
       * single-input operation sends is unchanged.
       */
      extra?: string[];
      /** One-based page numbers, every page exactly once. `reorder` only. */
      order?: number[];
      /** One-based page numbers. `rotate` only. */
      pages?: number[];
      /** A multiple of 90, negative or over 360. `rotate` only. */
      degrees?: number;
    },
  ): Promise<Reply>;
  /**
   * Rotate every page by 90 and report the rotations of the result.
   *
   * Three operations, mirroring `core/burrow-ops/tests/conformance.rs`: read the count,
   * rotate `1..=count`, read the rotations back out of the emitted bytes. The composition is
   * the harness's rather than the binding's — see the implementation.
   */
  rotateEveryPage(
    base64: string,
    options?: {
      password?: string | null;
      limits?: Partial<HarnessLimits>;
    },
  ): Promise<Reply>;

  /**
   * Reverse the document's page order and report the rotations of the result.
   *
   * The mirror of {@link rotateEveryPage}, and the same three-operation shape. See
   * `Operation::Reorder` on the Rust side for why the rotations are the observable.
   */
  reverseEveryPage(
    base64: string,
    options?: {
      password?: string | null;
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
  /** The recycling floor `burrow_types` defines, as a decimal string. Empty before init. */
  minConvergingMemoryBytes(): string;
  /** `Limits::DEFAULT` as Rust reports it, or `null` before a worker has initialised. */
  coreDefaultLimits(): Record<string, number> | null;
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
