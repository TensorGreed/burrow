// When the page-picture strip's render worker ends (#199, D1; owner, 2026-10-15).
//
// ONE DOCUMENT PER WORKER. PDFium's heap holds the document and its bitmaps for as long as the
// worker lives and nothing reaches into it, so a worker never serves a second document: it ends
// when the document changes, when the page is left, and before an operation runs.
//
// NEVER MID-RENDER, UNLESS IT OVERRUNS. Terminating the worker while PDFium was rendering crashed
// Firefox's content process (#361), so an end asked for while a render is in flight waits for
// that render to settle -- and its result is discarded and zeroed on arrival, never shown for the
// new document. Two rules keep that wait short and the deadline honest (owner, 2026-10-16, (c)):
//
// - ONE BATCH AT A TIME. A request carries at most `RENDER_BATCH` pages (`batchOf`), and `pump`'s
//   loop asks again for the rest, so an end waits for at most one batch -- never for every tile
//   the viewport wants.
// - THE DEADLINE IS SILENCE, NOT DURATION. It is restarted whenever a page arrives (`progress`),
//   so a slow render that is still producing pages is not killed; one that has posted nothing for
//   `END_DEADLINE_MS` is ended anyway, because a hung render must not keep the previous document's
//   heap alive, and the crash risk is the lesser cost.

/**
 * How long a waiting end lets the render in flight go WITHOUT A PAGE before terminating it
 * regardless: 3 seconds.
 *
 * WHAT IT IS MEASURED AGAINST. Not the whole request: the clock restarts at every page that
 * arrives (`EndGate.progress`). So it is set against ONE page, and one honest heavy page takes
 * about 1.2-1.5 s, measured in S5's round 1 -- three seconds is about twice that. Before
 * 2026-10-16 it ran from the end's request to the end of a request carrying every waiting tile,
 * and an honest document could still be rendering when it fired, terminating PDFium mid-render:
 * the path #361 records crashing Firefox. That path is still reachable, by a single page that
 * takes more than three seconds; it is no longer reached by a document that is merely long.
 */
export const END_DEADLINE_MS = 3_000;

/**
 * The most pages one render request carries: 4 (owner, 2026-10-16: 1-4).
 *
 * The bound on how long an end waits: a request in flight is at most this many pages, and
 * `pump`'s loop stops at the generation change, so the previous document's worker renders at
 * most one batch after it was asked to end. Four rather than one, because every request is a
 * round trip and a re-read of the page tree.
 */
export const RENDER_BATCH = 4;

/** The pages the next request asks for: the first `RENDER_BATCH` of those outstanding. */
export function batchOf(outstanding: readonly number[]): number[] {
  return outstanding.slice(0, RENDER_BATCH);
}

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
  /** A page arrived from the render in flight: a waiting end's deadline starts again. */
  progress(): void;
}

export function createEndGate(deps: EndGateDeps): EndGate {
  let waiting = false;
  let timer: unknown = null;

  const stopTimer = () => {
    if (timer !== null) deps.clearTimer(timer);
    timer = null;
  };
  // THE DEADLINE: a render that posts nothing for `END_DEADLINE_MS` is ended anyway.
  const arm = () => {
    stopTimer();
    timer = deps.setTimer(() => {
      timer = null;
      if (!waiting) return;
      waiting = false;
      deps.discard();
    }, END_DEADLINE_MS);
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
      arm();
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
    progress() {
      if (waiting) arm();
    },
  };
}

/** Zero a render's pixels that arrived for a document the strip no longer shows (D4). */
export function discardStale(pixels: ArrayBuffer): void {
  new Uint8Array(pixels).fill(0);
}
