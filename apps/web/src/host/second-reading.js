// Redaction with its second reading, from the page's side (#206's web half).
//
// The redaction worker holds no PDFium, so the reading is taken in a FRESH render worker, one per
// redaction, and judged in the redaction worker. The page's part is plumbing and nothing more:
//
//   - it opens a `MessageChannel` and hands one end to each worker, TRANSFERRED, so the two talk
//     directly and the page never holds either reading or either document copy;
//   - it may END a reading -- by its watchdog, or by a person stopping, which terminates the
//     workers -- and it has no way to CONFIRM one (owner option A, design review);
//   - it terminates the render worker as soon as the redaction worker's verdict is in, whatever
//     the render worker is doing (#199: one document per worker; design review L4, L5).
//
// The render host's own watchdog bounds the reading (design review L5): the render worker is
// terminated at its deadline even mid-`FPDFText_LoadPage`, where nothing else can stop it, and the
// redaction worker, which stopped waiting earlier, has already refused.
//
// THE COST OF THAT, STATED (owner's ruling for S5; security review of #206's web half). The
// redaction worker stops waiting after at most 10 s, and this then disposes the render host --
// TERMINATING a render worker that may be inside `FPDFText_LoadPage`, in a heap holding the
// document. That is the #361 class: a worker killed mid-call, whose heap is not wiped on the way
// out. What bounds it is that the worker is fresh, holds this one document and is never reused, so
// the heap goes with it; what it does not do is zero that heap first.

import { CANCELLED } from "./worker-host.js";

/**
 * @typedef {{ run: (message: object, options?: object) => Promise<any>, dispose: () => void }} Host
 */

/**
 * Redact `message` on `redact`, with its second reading taken on a fresh render host.
 *
 * @param {Host} redact the redaction worker's host
 * @param {() => Promise<Host>} freshRender builds a NEW render host for this one reading
 * @param {Record<string, any>} message the `redact` request
 * @param {{ maxDurationMs?: number, isLive?: () => boolean }} [options] `isLive` is the page's
 *   own run check, asked once the render host is built: a person who stopped, or a page that
 *   left, while that build was in flight gets a `Cancelled` reply, and no redaction is queued.
 * @returns {Promise<any>} the redaction worker's reply: its output only on an agreeing reading
 */
export async function redactWithSecondReading(redact, freshRender, message, options = {}) {
  const { isLive, ...hostOptions } = options;
  const render = await freshRender();
  // STOPPED DURING THE BUILD. The page's Stop discards the redaction worker, which settles what
  // is in flight and what is queued -- and at this point nothing is either: `redact.run` has not
  // been called, so a run enqueued now would start after the Stop and finish an operation the
  // person had already stopped (the defect worker-host.js's `cancellations` records as fixed,
  // reached around it). So the run is checked again here, and ends before any worker spawns.
  if (isLive && !isLive()) {
    render.dispose();
    return cancelledBeforeItStarted();
  }
  const channel = new MessageChannel();
  // STARTED ALONGSIDE THE REDACTION, so the render worker's start-up overlaps it rather than
  // eating the redaction worker's wait. Its answer goes over the port, not here.
  // ITS FAILURE IS NOBODY'S TO READ, so it is caught where it is made: the redaction worker's
  // verdict is the outcome, and a render host that dies first is a reading that never arrives.
  void render
    .run(
      { op: "second_reading_port", port: channel.port2, limits: message.limits },
      { maxDurationMs: hostOptions.maxDurationMs, transfer: [channel.port2] },
    )
    .catch(() => {});
  try {
    return await redact.run(
      { ...message, port: channel.port1 },
      { ...hostOptions, transfer: [channel.port1] },
    );
  } finally {
    // ENDED WITH THE VERDICT, whatever it was (#199). The render worker's own reply carries
    // nothing; it is waited for by nobody.
    render.dispose();
  }
}

/**
 * The reply a run stopped before it reached any worker gets: the host's own `Cancelled`, in the
 * host's reply shape, so the island treats it as every other stop.
 */
function cancelledBeforeItStarted() {
  return {
    ok: false,
    kind: CANCELLED,
    fatal: false,
    failedInput: -1,
    innerKind: "",
    output: null,
    message: "cancelled before it started",
    pages: 0,
    limit: "",
    stage: "",
    requested: "0",
    allowed: "0",
    recycle: false,
    engineHeapBytes: "0",
  };
}
