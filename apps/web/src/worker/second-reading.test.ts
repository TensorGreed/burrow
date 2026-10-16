// #206's web half, both workers' halves: the second reading of a redacted page is taken in a fresh
// render worker and judged in the redaction worker, over a `MessagePort` the page hands each.
//
// What is held here, each against the real bundle text and each shown to fail by a mutation that is
// asserted to have applied:
//
// - EXACT AND TRANSFERRED (design review M2): every buffer either worker posts on the port is an
//   exact-length `ArrayBuffer`, listed in the transfer list, and detached on the sender's side; the
//   receiver gets exactly the bytes, no more.
// - ONE ANSWER PER NONCE: an answer for any other nonce is dropped; a failure, a timeout or an
//   oversized answer (design review L3, checked BEFORE the copy in) aborts and refuses -- the
//   redaction worker never confirms anything but the answer for its own nonce.
// - THE FULL LIMITS cross, with the operation's remaining budget as the duration.
// - THE RENDER WORKER takes a reading ONLY on the port, behind the policy guard (M3), hashes what
//   it was sent with the platform's SHA-256, and zeroes what it received.
//
// The browser half -- real workers, a real port, real PDFium -- is `e2e/redaction-emission.spec.ts`
// and `e2e/redaction-differential.spec.ts`.

import { createHash } from "node:crypto";
import { describe, expect, it } from "vitest";

import { REDACT_MAIN, RENDER_MAIN, load, replyShape } from "./test-scope";

const LIMITS = {
  maxInputBytes: 1_000,
  maxMemoryBytes: 2_000,
  maxDurationMs: 12_000,
  maxPages: 30,
  maxPixels: 40,
};

/** The real platform pieces a port needs, which a `vm` context does not have. */
function platform(scope: Record<string, unknown>) {
  Object.assign(scope, {
    ArrayBuffer,
    Uint8Array,
    MessagePort,
    MessageChannel,
    setTimeout,
    clearTimeout,
    crypto: globalThis.crypto,
    performance,
  });
}

/** `source` with `from` replaced by `to`, asserting it was there exactly once. */
function mutated(source: string, from: string, to: string): string {
  const at = source.indexOf(from);
  expect(at, `the mutation's anchor is not in the bundle: ${from}`).not.toBe(-1);
  expect(source.indexOf(from, at + 1), `the anchor is not unique: ${from}`).toBe(-1);
  const out = source.slice(0, at) + to + source.slice(at + from.length);
  expect(out).not.toBe(source);
  return out;
}

const tick = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

/** The next message on `port`, or `null` after `ms`. */
function next(port: MessagePort, ms = 1_000): Promise<any> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(null), ms);
    port.onmessage = (event) => {
      clearTimeout(timer);
      port.onmessage = null;
      resolve(event.data);
    };
  });
}

// ---- the redaction worker --------------------------------------------------------------------

interface Redaction {
  posted: Record<string, unknown>[];
  confirms: { nonce: bigint; answer: number[]; view: Uint8Array }[];
  aborts: bigint[];
  handedOut: Uint8Array[];
  render: MessagePort;
  done: Promise<void>;
}

/**
 * Start one redaction in a copy of the redaction bundle, with the render worker's end of the port
 * held by the test. `remainingMs` is what phase 1 reports as left of the budget.
 */
function redaction(
  source = REDACT_MAIN,
  { remainingMs = 5_000n, begunOk = true, maxAnswer = 64 } = {},
): Redaction {
  const h = load(source);
  platform(h.scope);
  // The scope holds qpdf's start-up open until told; nothing here is about start-up.
  h.releaseQpdf();
  const confirms: Redaction["confirms"] = [];
  const aborts: bigint[] = [];
  const handedOut: Uint8Array[] = [];
  const wb = h.scope["wasm_bindgen"] as Record<string, unknown>;
  wb["redactBegin"] = () => ({
    reply: () => ({ ...replyShape(), ok: begunOk, kind: begunOk ? "" : "InvalidArgument" }),
    nonce: 7n,
    remainingMs,
    takeInput: () => {
      // A VIEW INTO A LARGER BUFFER, as a careless binding could hand one back: it must be copied
      // to an exact buffer before it is posted, never posted whole.
      const view = new Uint8Array(new ArrayBuffer(16), 4, 3);
      view.set([1, 2, 3]);
      handedOut.push(view);
      return view;
    },
    takeOutput: () => {
      const view = new Uint8Array([4, 5, 6, 7]);
      handedOut.push(view);
      return view;
    },
    free: () => {},
  });
  wb["redactConfirm"] = (nonce: bigint, answer: Uint8Array) => {
    confirms.push({ nonce, answer: [...answer], view: answer });
    return { ...replyShape(), ok: true };
  };
  wb["redactAbort"] = (nonce: bigint) => {
    aborts.push(nonce);
    return { ...replyShape(), ok: false, kind: "OutputRejected" };
  };
  wb["maxSecondReadingAnswer"] = () => maxAnswer;
  const channel = new MessageChannel();
  const done = h.send({
    id: 1,
    op: "redact",
    page: 2,
    covered: [2],
    region: { left: 0, top: 0, width: 1, height: 1 },
    limits: LIMITS,
    blob: { size: 3, arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer },
    port: channel.port1,
  });
  return { posted: h.posted, confirms, aborts, handedOut, render: channel.port2, done };
}

/** The one reply on the page's channel that names the request, after the ack. */
const terminal = (posted: Record<string, unknown>[]) =>
  posted.filter((m) => m["id"] === 1 && m["ack"] !== true);

describe("the redaction worker's half of the second reading", () => {
  it("posts exact, transferred copies with the full limits, and confirms its own nonce's answer", async () => {
    const r = redaction();
    const read = await next(r.render);
    expect(read, "no reading was asked for").not.toBeNull();
    expect(read.type).toBe("read");
    expect(read.nonce).toBe(7n);
    expect(read.page, "the page is the 0-based index of the request's 1-based page").toBe(1);
    // EXACTLY THE BYTES, in buffers of exactly their length -- not the 16-byte buffer behind the view.
    expect(read.input).toBeInstanceOf(ArrayBuffer);
    expect([...new Uint8Array(read.input)]).toEqual([1, 2, 3]);
    expect(read.input.byteLength).toBe(3);
    expect([...new Uint8Array(read.output)]).toEqual([4, 5, 6, 7]);
    // THE FULL LIMITS, the duration being what phase 1 says is left.
    expect(read.limits).toEqual({ ...LIMITS, maxDurationMs: 5_000 });
    // DETACHED here: the exact output buffer was transferred, so nothing of it stayed behind.
    expect(r.handedOut[1].buffer.byteLength, "the output was copied, not transferred").toBe(0);

    const answer = new Uint8Array([9, 8, 7]);
    r.render.postMessage({ type: "answer", nonce: 7n, answer: answer.buffer }, [answer.buffer]);
    await r.done;
    expect(r.confirms.map((c) => [c.nonce, c.answer])).toEqual([[7n, [9, 8, 7]]]);
    expect(r.aborts).toEqual([]);
    // #199: the answer is zeroed once Rust has it.
    expect([...r.confirms[0].view]).toEqual([0, 0, 0]);
    expect(terminal(r.posted)).toHaveLength(1);
    expect(terminal(r.posted)[0]["ok"]).toBe(true);
    r.render.close();
  });

  it("drops an answer for another nonce, and judges only its own", async () => {
    const r = redaction();
    await next(r.render);
    const foreign = new Uint8Array([1]);
    r.render.postMessage({ type: "answer", nonce: 8n, answer: foreign.buffer }, [foreign.buffer]);
    await tick(20);
    expect(r.confirms, "an answer for another nonce was judged").toEqual([]);
    const own = new Uint8Array([2]);
    r.render.postMessage({ type: "answer", nonce: 7n, answer: own.buffer }, [own.buffer]);
    await r.done;
    expect(r.confirms.map((c) => c.answer)).toEqual([[2]]);
    r.render.close();
  });

  it("refuses when no answer arrives within the budget left, and confirms nothing", async () => {
    const r = redaction(REDACT_MAIN, { remainingMs: 30n });
    await next(r.render);
    await r.done;
    expect(r.aborts).toEqual([7n]);
    expect(r.confirms).toEqual([]);
    expect(terminal(r.posted)[0]["ok"]).toBe(false);
    r.render.close();
  });

  it("refuses on the render worker's failure, which carries no nonce", async () => {
    const r = redaction();
    await next(r.render);
    r.render.postMessage({ type: "failed", kind: "Internal" });
    await r.done;
    expect(r.aborts).toEqual([7n]);
    expect(r.confirms).toEqual([]);
    r.render.close();
  });

  it("refuses an answer longer than Rust takes BEFORE copying it in (L3)", async () => {
    const r = redaction(REDACT_MAIN, { maxAnswer: 4 });
    await next(r.render);
    const long = new Uint8Array(5);
    r.render.postMessage({ type: "answer", nonce: 7n, answer: long.buffer }, [long.buffer]);
    await r.done;
    expect(r.confirms, "an oversized answer reached Rust").toEqual([]);
    expect(r.aborts).toEqual([7n]);
    r.render.close();
  });

  it("asks for no reading when phase 1 refused, and posts that refusal", async () => {
    const r = redaction(REDACT_MAIN, { begunOk: false });
    expect(await next(r.render, 100), "a refused redaction asked for a reading").toBeNull();
    await r.done;
    expect(r.confirms).toEqual([]);
    expect(terminal(r.posted)[0]["kind"]).toBe("InvalidArgument");
    r.render.close();
  });

  // ---- each property, shown to fail ----------------------------------------------------------

  it("SHOWN TO FAIL: without the nonce check, a foreign answer is judged", async () => {
    const source = mutated(REDACT_MAIN, "data.nonce !== nonce", "false");
    const r = redaction(source);
    await next(r.render);
    const foreign = new Uint8Array([1]);
    r.render.postMessage({ type: "answer", nonce: 8n, answer: foreign.buffer }, [foreign.buffer]);
    await r.done;
    expect(r.confirms.map((c) => c.answer)).toEqual([[1]]);
    r.render.close();
  });

  it("SHOWN TO FAIL: without the wait's timer, a missing answer is never refused", async () => {
    const source = mutated(
      REDACT_MAIN,
      "const timer = setTimeout(() => settle(null), timeoutMs);",
      "const timer = 0;",
    );
    const r = redaction(source, { remainingMs: 30n });
    await next(r.render);
    await tick(200);
    expect(r.aborts, "the mutant refused anyway: the timer is not what refuses").toEqual([]);
    expect(terminal(r.posted)).toEqual([]);
    r.render.close();
  });

  it("SHOWN TO FAIL: without the length check, an oversized answer reaches Rust", async () => {
    const source = mutated(
      REDACT_MAIN,
      "bytes.byteLength > wasm_bindgen.maxSecondReadingAnswer()",
      "false",
    );
    const r = redaction(source, { maxAnswer: 4 });
    await next(r.render);
    const long = new Uint8Array(5);
    r.render.postMessage({ type: "answer", nonce: 7n, answer: long.buffer }, [long.buffer]);
    await r.done;
    expect(r.confirms).toHaveLength(1);
    r.render.close();
  });

  it("SHOWN TO FAIL: posting a view's whole buffer sends more than the bytes", async () => {
    const source = mutated(
      REDACT_MAIN,
      "    view.byteOffset === 0 && view.byteLength === view.buffer.byteLength\n      ? view\n      : new Uint8Array(view);",
      "    view;\n  if (exact.byteOffset !== 0) return /** @type {ArrayBuffer} */ (exact.buffer);",
    );
    const r = redaction(source);
    const read = await next(r.render);
    expect(read.input.byteLength, "the mutant sent only the bytes").toBe(16);
    r.render.close();
  });

  it("SHOWN TO FAIL: posting without the transfer list leaves the copy behind, and is refused", async () => {
    const source = mutated(
      REDACT_MAIN,
      "  port.postMessage(message, buffers);",
      "  port.postMessage(message);",
    );
    const r = redaction(source);
    await next(r.render);
    await r.done;
    // THE CHECK BEHIND THE TRANSFER REFUSES: a buffer still attached is an internal failure.
    expect(r.handedOut[1].buffer.byteLength, "the mutant transferred anyway").toBe(4);
    expect(terminal(r.posted)[0]["kind"]).toBe("Internal");
    expect(r.confirms).toEqual([]);
    r.render.close();
  });
});

// ---- the render worker -----------------------------------------------------------------------

interface Reading {
  posted: Record<string, unknown>[];
  calls: { input: Uint8Array; output: Uint8Array; page: number; dIn: number[]; dOut: number[] }[];
  durations: bigint[];
  redaction: MessagePort;
  scope: Record<string, unknown>;
  done: Promise<void>;
}

/** Hand a copy of the render bundle one end of a port, as the page does, keeping the other. */
async function reading(source = RENDER_MAIN, policed = true): Promise<Reading> {
  const h = load(source, policed);
  platform(h.scope);
  const calls: Reading["calls"] = [];
  const durations: bigint[] = [];
  const wb = h.scope["wasm_bindgen"] as Record<string, unknown>;
  wb["WebLimits"] = class {
    constructor(_input: bigint, _memory: bigint, duration: bigint) {
      durations.push(duration);
    }
  };
  wb["secondReading"] = (
    input: Uint8Array,
    output: Uint8Array,
    page: number,
    dIn: Uint8Array,
    dOut: Uint8Array,
  ) => {
    calls.push({ input, output, page, dIn: [...dIn], dOut: [...dOut] });
    // A VIEW INTO A LARGER BUFFER, like the redaction side's case: the answer must leave exact.
    const answer = new Uint8Array(new ArrayBuffer(32), 8, 2);
    answer.set([0xaa, 0xbb]);
    return answer;
  };
  const channel = new MessageChannel();
  // NOT AWAITED: the global channel's operation lasts until the port has been served.
  const done = h.send({ id: 5, op: "second_reading_port", port: channel.port2, limits: LIMITS });
  return { posted: h.posted, calls, durations, redaction: channel.port1, scope: h.scope, done };
}

const sha256 = (bytes: number[]) => [...createHash("sha256").update(Buffer.from(bytes)).digest()];

/** What the redaction worker sends, as it sends it. */
function sendRead(port: MessagePort, extra: Record<string, unknown> = {}) {
  const input = new Uint8Array([1, 2, 3]).buffer;
  const output = new Uint8Array([4, 5]).buffer;
  port.postMessage(
    {
      type: "read",
      nonce: 7n,
      page: 0,
      input,
      output,
      password: undefined,
      limits: LIMITS,
      ...extra,
    },
    [input, output],
  );
}

describe("the render worker's half of the second reading", () => {
  it("reads what the port sent, hashed with the platform's SHA-256, and answers exact on the port", async () => {
    const r = await reading();
    sendRead(r.redaction);
    const answer = await next(r.redaction);
    expect(answer.type).toBe("answer");
    expect(answer.nonce).toBe(7n);
    expect([...new Uint8Array(answer.answer)], "the answer is not exactly the bytes").toEqual([
      0xaa, 0xbb,
    ]);
    expect(r.calls).toHaveLength(1);
    const [call] = r.calls;
    expect(call.page).toBe(0);
    // THE DIGESTS ARE OF THE BYTES THAT WERE READ.
    expect(call.dIn).toEqual(sha256([1, 2, 3]));
    expect(call.dOut).toEqual(sha256([4, 5]));
    // #199: zeroed once the module had them.
    expect([...call.input]).toEqual([0, 0, 0]);
    expect([...call.output]).toEqual([0, 0]);
    // A DURATION NO LONGER THAN THE REQUEST'S, and bounded by this worker's own ceiling.
    expect(r.durations).toHaveLength(1);
    expect(r.durations[0] <= 10_000n).toBe(true);
    // The global channel's terminal reply carries nothing: the answer went on the port.
    await r.done;
    const global = r.posted.filter((m) => m["id"] === 5 && m["ack"] !== true);
    expect(global).toHaveLength(1);
    expect(global[0]["ok"]).toBe(true);
    expect(Object.values(global[0]).some((v) => v instanceof ArrayBuffer)).toBe(false);
  });

  it("answers one request and closes the port", async () => {
    const r = await reading();
    sendRead(r.redaction);
    await next(r.redaction);
    sendRead(r.redaction);
    expect(await next(r.redaction, 100), "a second request on the port was answered").toBeNull();
    expect(r.calls).toHaveLength(1);
  });

  /** Two messages back to back -- the reading's request, then anything -- and the first reply. */
  async function backToBack(source: string) {
    const r = await reading(source);
    sendRead(r.redaction);
    r.redaction.postMessage({ copy: true });
    const first = await next(r.redaction);
    await tick(20);
    return { r, first };
  }

  it("handles only the first message on its port, however soon a second follows", async () => {
    const { r, first } = await backToBack(RENDER_MAIN);
    expect(first.type, "the second message was answered in the first one's place").toBe("answer");
    expect(r.calls).toHaveLength(1);
  });

  it("SHOWN TO FAIL: with the handler left on, the second message is answered first", async () => {
    const source = mutated(
      RENDER_MAIN,
      "  port.onmessage = async (event) => {\n    port.onmessage = null;\n",
      "  port.onmessage = async (event) => {\n",
    );
    const { first } = await backToBack(source);
    expect(first).toEqual({ type: "failed", kind: "InvalidArgument" });
  });

  it("refuses a malformed request on the port without reading anything", async () => {
    const r = await reading();
    sendRead(r.redaction, { input: "not bytes" });
    const reply = await next(r.redaction);
    expect(reply).toEqual({ type: "failed", kind: "InvalidArgument" });
    expect(r.calls).toEqual([]);
  });

  it("takes no reading on the global channel: only the port is a door", async () => {
    const h = load(RENDER_MAIN);
    platform(h.scope);
    const read = new Uint8Array([1]).buffer;
    await h.send({ id: 9, op: "second_reading", input: read, output: read, limits: LIMITS });
    await h.send({ id: 10, op: "read", input: read, output: read, limits: LIMITS });
    const replies = h.posted.filter((m) => m["id"] === 9 || m["id"] === 10);
    expect(replies.map((m) => m["message"])).toEqual(["unknown operation", "unknown operation"]);
  });

  it("refuses on the port when no policy is in force, before reading", async () => {
    const h = load(RENDER_MAIN, false);
    platform(h.scope);
    const channel = new MessageChannel();
    let handled = 0;
    (h.scope["servePort"] as (port: MessagePort, handle: () => Promise<void>) => void)(
      channel.port2,
      async () => {
        handled += 1;
      },
    );
    sendRead(channel.port1);
    expect(await next(channel.port1)).toEqual({ type: "failed", kind: "Internal" });
    expect(handled).toBe(0);
    channel.port1.close();
  });

  it("SHOWN TO FAIL: without the zeroing, what was read stays in this worker", async () => {
    const source = mutated(RENDER_MAIN, "for (const view of received) view.fill(0);", "");
    const r = await reading(source);
    sendRead(r.redaction);
    await next(r.redaction);
    expect([...r.calls[0].input]).toEqual([1, 2, 3]);
  });

  it("SHOWN TO FAIL: an answer posted as its view's whole buffer carries more than the answer", async () => {
    const source = mutated(
      RENDER_MAIN,
      "    view.byteOffset === 0 && view.byteLength === view.buffer.byteLength\n      ? view\n      : new Uint8Array(view);",
      "    view;\n  if (exact.byteOffset !== 0) return /** @type {ArrayBuffer} */ (exact.buffer);",
    );
    const r = await reading(source);
    sendRead(r.redaction);
    const answer = await next(r.redaction);
    expect(answer.answer.byteLength).toBe(32);
  });
});
