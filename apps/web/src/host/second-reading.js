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

/**
 * @typedef {{ run: (message: object, options?: object) => Promise<any>, dispose: () => void }} Host
 */

/**
 * Redact `message` on `redact`, with its second reading taken on a fresh render host.
 *
 * @param {Host} redact the redaction worker's host
 * @param {() => Promise<Host>} freshRender builds a NEW render host for this one reading
 * @param {Record<string, any>} message the `redact` request
 * @param {{ maxDurationMs?: number }} [options]
 * @returns {Promise<any>} the redaction worker's reply: its output only on an agreeing reading
 */
export async function redactWithSecondReading(redact, freshRender, message, options = {}) {
  const channel = new MessageChannel();
  const render = await freshRender();
  // STARTED ALONGSIDE THE REDACTION, so the render worker's start-up overlaps it rather than
  // eating the redaction worker's wait. Its answer goes over the port, not here.
  const reading = render.run(
    { op: "second_reading_port", port: channel.port2, limits: message.limits },
    { maxDurationMs: options.maxDurationMs, transfer: [channel.port2] },
  );
  try {
    return await redact.run(
      { ...message, port: channel.port1 },
      { ...options, transfer: [channel.port1] },
    );
  } finally {
    // ENDED WITH THE VERDICT, whatever it was (#199). The render worker's own reply carries
    // nothing; it is waited for by nobody.
    render.dispose();
    reading.catch(() => {});
  }
}
