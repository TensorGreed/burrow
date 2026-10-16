// The page's half of redaction's second reading (#206's web half): plumbing, and nothing more.
//
// What it must hold, each asserted here: the two ports go one to each worker and are TRANSFERRED,
// so the page keeps neither; the render host is a fresh one per redaction and is disposed with the
// verdict, whatever the verdict was; and what the page gets back is the redaction worker's reply,
// untouched -- the page has no way to make a refusal into a success.

import { describe, expect, test } from "vitest";

import { redactWithSecondReading } from "./second-reading.js";

interface Call {
  message: Record<string, unknown>;
  options: { transfer?: Transferable[]; maxDurationMs?: number };
}

/** A host that records what it is asked, and answers what the case says. */
function fakeHost(answer: (call: Call) => Promise<unknown>) {
  const calls: Call[] = [];
  let disposed = 0;
  return {
    calls,
    disposed: () => disposed,
    run(message: object, options: object = {}) {
      const call = {
        message: message as Record<string, unknown>,
        options: options as Call["options"],
      };
      calls.push(call);
      return answer(call);
    },
    dispose() {
      disposed += 1;
    },
  };
}

const REQUEST = { op: "redact", page: 1, covered: [1], limits: { maxDurationMs: 12_000 } };

describe("redaction with its second reading, from the page", () => {
  test("hands one port to each worker, transferred, and keeps neither", async () => {
    const redact = fakeHost(async () => ({ ok: true }));
    const render = fakeHost(() => new Promise(() => {}));
    await redactWithSecondReading(redact, async () => render, REQUEST, { maxDurationMs: 12_000 });

    const [toRedact] = redact.calls;
    const [toRender] = render.calls;
    expect(toRender.message.op).toBe("second_reading_port");
    const redactPort = toRedact.message.port;
    const renderPort = toRender.message.port;
    expect(redactPort).toBeInstanceOf(MessagePort);
    expect(renderPort).toBeInstanceOf(MessagePort);
    expect(redactPort).not.toBe(renderPort);
    // TRANSFERRED: each port is in its own message's transfer list, and only there.
    expect(toRedact.options.transfer).toEqual([redactPort]);
    expect(toRender.options.transfer).toEqual([renderPort]);
    // AND THE REQUEST IS THE PAGE'S, with the port added and nothing else changed.
    const { port: _port, ...rest } = toRedact.message;
    expect(rest).toEqual(REQUEST);
    (redactPort as MessagePort).close();
    (renderPort as MessagePort).close();
  });

  test("builds a fresh render host for each redaction, and disposes it with the verdict", async () => {
    const redact = fakeHost(async () => ({ ok: true }));
    const built: ReturnType<typeof fakeHost>[] = [];
    const fresh = async () => {
      const render = fakeHost(() => new Promise(() => {}));
      built.push(render);
      return render;
    };
    await redactWithSecondReading(redact, fresh, REQUEST);
    await redactWithSecondReading(redact, fresh, REQUEST);
    expect(built).toHaveLength(2);
    expect(built.map((render) => render.disposed())).toEqual([1, 1]);
  });

  test("disposes the render host on a refusal too, and returns the refusal untouched", async () => {
    const refusal = { ok: false, kind: "OutputRejected", message: "x" };
    const redact = fakeHost(async () => refusal);
    const render = fakeHost(async () => ({ ok: true }));
    const reply = await redactWithSecondReading(redact, async () => render, REQUEST);
    expect(reply).toBe(refusal);
    expect(render.disposed()).toBe(1);
  });

  test("disposes the render host when the redaction host itself throws", async () => {
    const redact = fakeHost(async () => {
      throw new Error("host gone");
    });
    const render = fakeHost(() => new Promise(() => {}));
    await expect(redactWithSecondReading(redact, async () => render, REQUEST)).rejects.toThrow();
    expect(render.disposed()).toBe(1);
  });

  test("a render host that fails does not turn into a success: the reply is the redaction worker's", async () => {
    const refusal = { ok: false, kind: "OutputRejected" };
    const redact = fakeHost(async () => refusal);
    const render = fakeHost(async () => ({ ok: false, kind: "Internal" }));
    expect(await redactWithSecondReading(redact, async () => render, REQUEST)).toBe(refusal);
  });
});
