// When the page-picture strip's render worker ends (#199, D1; owner, 2026-10-15).
//
// ONE DOCUMENT PER WORKER. PDFium's heap holds the document and its bitmaps for as long as the
// worker lives and nothing reaches into it, so a worker never serves a second document: it ends
// when the document changes, when the page is left, and before an operation runs.
//
// NEVER MID-RENDER, UNLESS IT OVERRUNS. Terminating the worker while PDFium was rendering crashed
// Firefox's content process (#361), so an end asked for while a render is in flight waits for
// that render to settle -- and its result is discarded and zeroed on arrival, never shown for the
// new document. A render that has not settled within `END_DEADLINE_MS` is ended anyway: a hung
// render must not keep the previous document's heap alive, and the crash risk is the lesser cost.

/**
 * How long an end waits for the render in flight before terminating it regardless: 3 seconds.
 *
 * WHAT IT IS MEASURED AGAINST, CORRECTED (S5's round 1, both reviewers). A render in flight is
 * not one page: `PageThumbnails`' `drawRun` sends every waiting tile the strip wants -- tens of
 * pages on a desktop viewport -- as ONE request, and one honest heavy page takes about 1.2-1.5 s.
 * So an honest document can still be rendering when this fires, and the end then terminates
 * PDFium mid-render: the path #361 records crashing Firefox. The deadline bounds how long the
 * previous document's worker lives; it does not guarantee the render it cuts short was hung.
 * Whether to cap each request at a small batch, or re-arm the deadline as each page arrives, is
 * with the owner (2026-10-15); neither is built.
 */
export const END_DEADLINE_MS = 3_000;

export interface EndGateDeps {
  /** Terminate the render worker. */
  discard: () => void;
  setTimer: (fn: () => void, ms: number) => unknown;
  clearTimer: (handle: unknown) => void;
}

export interface EndGate {
  /** Ask for the worker to end: now if nothing is rendering, else when the render settles. */
  request(rendering: boolean): void;
  /** The render in flight has settled: end the worker if an end was asked for. */
  settled(): void;
  /** Whether an end is waiting for the render in flight. */
  pending(): boolean;
  /** Drop a waiting end, and its deadline, without ending anything: the caller ended it already. */
  cancel(): void;
}

export function createEndGate(deps: EndGateDeps): EndGate {
  let waiting = false;
  let timer: unknown = null;

  const stopTimer = () => {
    if (timer !== null) deps.clearTimer(timer);
    timer = null;
  };

  return {
    request(rendering) {
      if (!rendering) {
        stopTimer();
        waiting = false;
        deps.discard();
        return;
      }
      if (waiting) return;
      waiting = true;
      // THE DEADLINE: a render that does not settle is ended anyway.
      timer = deps.setTimer(() => {
        timer = null;
        if (!waiting) return;
        waiting = false;
        deps.discard();
      }, END_DEADLINE_MS);
    },
    settled() {
      if (!waiting) return;
      waiting = false;
      stopTimer();
      deps.discard();
    },
    pending: () => waiting,
    cancel() {
      waiting = false;
      stopTimer();
    },
  };
}

/** Zero a render's pixels that arrived for a document the strip no longer shows (D4). */
export function discardStale(pixels: ArrayBuffer): void {
  new Uint8Array(pixels).fill(0);
}
