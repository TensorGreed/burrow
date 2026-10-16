// Redaction's second reading travels over a `MessagePort` the page hands both workers (#206's web
// half). Concatenated after `worker-protocol.js` in the render and redaction bundles only; the base
// bundle never takes a reading.
//
// A port is a second front door, so every message on it goes through what the global channel's
// does: the fail-closed policy guard, the memoised init, and a catch that binds nothing and reports
// a fixed `Internal` (ADR 0009; design review M3).

/**
 * `view` as an exact-length, unshared `ArrayBuffer` to TRANSFER: never a view of wasm memory, which
 * would carry the whole heap (design review M2). Asserted, not assumed.
 *
 * @param {Uint8Array} view
 * @returns {ArrayBuffer}
 */
function exactBuffer(view) {
  const exact =
    view.byteOffset === 0 && view.byteLength === view.buffer.byteLength
      ? view
      : new Uint8Array(view);
  if (exact.byteOffset !== 0 || exact.byteLength !== exact.buffer.byteLength) {
    throw new Error("a buffer for the port is not exact");
  }
  return /** @type {ArrayBuffer} */ (exact.buffer);
}

/**
 * Post `message` on `port`, transferring `buffers`, and check each was detached: one that was not
 * is a second copy of a person's document left in this worker, and an internal failure.
 *
 * @param {MessagePort} port
 * @param {object} message
 * @param {ArrayBuffer[]} buffers
 */
function postTransferring(port, message, buffers) {
  port.postMessage(message, buffers);
  if (buffers.some((buffer) => buffer.byteLength !== 0)) {
    throw new Error("a buffer was not transferred");
  }
}

/**
 * Serve the FIRST message on `port` with `handle`, behind the guard, the init and the catch.
 *
 * The handler comes off before anything is awaited, so a second message is never handled.
 * Measured: left on, a second message posted straight after the reading's request was answered,
 * with a failure, while the first was still being hashed -- in Firefox and WebKit.
 *
 * @param {MessagePort} port
 * @param {(data: any, port: MessagePort) => Promise<void>} handle
 */
function servePort(port, handle) {
  port.onmessage = async (event) => {
    port.onmessage = null;
    const policy = await POLICED;
    if (!policy.policed) {
      port.postMessage({ type: "failed", kind: "Internal" });
      port.close();
      return;
    }
    try {
      await ensureReady();
      await handle(event.data, port);
    } catch {
      try {
        port.postMessage({ type: "failed", kind: "Internal" });
      } catch {
        // The port is gone; there is nobody to tell.
      }
      port.close();
    }
  };
}

/**
 * The first message on `port` that `accept` takes, or `null` on a timeout, a refused policy or a
 * throw. Messages `accept` refuses are dropped. The port is closed however it ends.
 *
 * @template T
 * @param {MessagePort} port
 * @param {(data: any) => T | null} accept
 * @param {number} timeoutMs
 * @returns {Promise<T | null>}
 */
function awaitPortReply(port, accept, timeoutMs) {
  return new Promise((resolve) => {
    let settled = false;
    /** @param {T | null} value */
    const settle = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      port.onmessage = null;
      port.close();
      resolve(value);
    };
    const timer = setTimeout(() => settle(null), timeoutMs);
    port.onmessage = async (event) => {
      try {
        const policy = await POLICED;
        if (!policy.policed) {
          settle(null);
          return;
        }
        const taken = accept(event.data);
        if (taken !== null) settle(taken);
      } catch {
        settle(null);
      }
    };
  });
}
