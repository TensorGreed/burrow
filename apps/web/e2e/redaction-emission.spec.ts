// R8's named check (ADR 0006, #137): redaction's output is ONE value, posted only after the Rust
// call returned verified success. Progress may report position; it may not emit content.
//
// Every message between the page and redaction's worker is recorded by the harness before the
// host reads it, so this sees what the page's thread was handed -- not what the host chose to
// keep -- and it reads the log only after the worker has gone quiet. The rules are DENY BY
// DEFAULT: an exact shape per message, measured on the real worker (see `r8Violations`). It is
// SHOWN TO FAIL rule by rule: by copies of the worker that leak, each mutation asserted to have
// applied, and by hand-written logs for the rules no copy of this worker can reach.
//
// WHAT IT DOES NOT SEE:
// - WHAT THE REPLY'S OWN `message` AND `report` SAY. They are Rust's prose; an error quoting the
//   document would pass. The typed-error rule holds that, not this spec.
// - A message deferred by more than `SETTLE_MS`.
// - A `MessagePort` the page never received. One transferred to the page is caught; #206's
//   second-engine reading will add a worker-to-worker port, and ADR 0029's 2026-09-27 amendment
//   requires this spec to follow it.

import { expect, test } from "@playwright/test";

import type { RedactionMessage } from "../src/host/harness-api";
import { openHarness } from "./harness";
import {
  THE_CALL,
  cleanLog,
  onlyRequest,
  r8Violations,
  redactAndSettle,
} from "./redaction-protocol";

test("a successful redaction posts its document once, in the terminal reply", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({}));
  const { reply, log, nonce } = await redactAndSettle(page, 1);
  expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(true);

  // THE RECORDER SAW THE DOCUMENT: one that recorded nothing would pass every "no message carries
  // bytes" rule below.
  expect(
    log.filter((m) => m.sent === null && m.bytes > 0).length,
    "the recorder never saw the document",
  ).toBe(1);
  expect(r8Violations(log, onlyRequest(log), nonce)).toEqual([]);
});

test("a refused redaction posts no bytes at all", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({}));
  const { reply, log, nonce } = await redactAndSettle(page, 2); // one past the end
  expect(reply.ok).toBe(false);

  expect(
    log.some((m) => m.sent === null && m.ok === false),
    "the recorder never saw the worker's refusal",
  ).toBe(true);
  expect(r8Violations(log, onlyRequest(log), nonce)).toEqual([]);
});

/** A planted leak before the call, and the finding R8 must report for it. */
const before = (leak: string) => ({ from: THE_CALL, to: `  ${leak}\n${THE_CALL}` });
const chunk = "bytes.subarray(0, 16)";
const latin1 = 'new TextDecoder("latin1").decode(bytes.subarray(0, 256))';
const BYTES = /carries bytes, and is not the terminal reply/;
const UNLISTED = /has a shape nobody listed/;

const LEAKS: { name: string; from: string; to: string; finding: RegExp }[] = [
  {
    name: "a chunk naming no request",
    ...before(`self.postMessage({ chunk: ${chunk} });`),
    finding: BYTES,
  },
  // The host takes a message under the request's id as its reply, so this one does not merely
  // leak -- it becomes the answer, and the real reply arrives after anyone looked.
  {
    name: "a chunk under the request's id",
    ...before(`self.postMessage({ id: request.id, chunk: ${chunk} });`),
    finding: BYTES,
  },
  {
    name: "a chunk under a foreign id",
    ...before(`self.postMessage({ id: request.id + 1000, chunk: ${chunk} });`),
    finding: BYTES,
  },
  {
    name: "a chunk whose id is undefined",
    ...before(`self.postMessage({ id: undefined, chunk: ${chunk} });`),
    finding: BYTES,
  },
  // Under a key the harness's own messages use: the first exemption was by prefix.
  {
    name: "a chunk under a __burrow key",
    ...before(`self.postMessage({ __burrow_chunk: ${chunk} });`),
    finding: BYTES,
  },
  {
    name: "a chunk as a forged settle echo",
    ...before(`self.postMessage({ __burrowSettled: ${chunk} });`),
    finding: BYTES,
  },
  {
    name: "a settle echo with the wrong nonce",
    ...before("self.postMessage({ __burrowSettled: 1 });"),
    finding: UNLISTED,
  },
  {
    name: "the input as a number array",
    ...before("self.postMessage({ progress: Array.from(bytes.subarray(0, 256)) });"),
    finding: UNLISTED,
  },
  {
    name: "the input as a string in a listed key",
    ...before(`self.postMessage({ starting: ${latin1} });`),
    finding: UNLISTED,
  },
  {
    name: "the input as an object's key",
    ...before(`self.postMessage({ starting: { [${latin1}]: true } });`),
    finding: /holds a run of 256/,
  },
  {
    name: "the input as a BigInt",
    ...before(
      `self.postMessage({ starting: BigInt("0x" + Array.from(bytes.subarray(0, 64), (b) => b.toString(16).padStart(2, "0")).join("")) });`,
    ),
    finding: UNLISTED,
  },
  {
    name: "the input as an Error",
    ...before(`self.postMessage({ starting: new Error(${latin1}) });`),
    finding: UNLISTED,
  },
  {
    name: "the input as a boxed String",
    ...before(`self.postMessage({ starting: new String(${latin1}) });`),
    finding: UNLISTED,
  },
  {
    name: "the input as ImageData",
    ...before(
      "self.postMessage({ starting: new ImageData(new Uint8ClampedArray(bytes.slice(0, 64)), 4, 4) });",
    ),
    finding: BYTES,
  },
  {
    name: "the input down a transferred stream",
    ...before(
      "{ const s = new ReadableStream({ start(c) { c.enqueue(bytes); c.close(); } }); self.postMessage({ starting: s }, [s]); }",
    ),
    finding: UNLISTED,
  },
  {
    name: "a port the input then goes down",
    ...before(
      "{ const c = new MessageChannel(); self.postMessage({ starting: true }, [c.port2]); c.port1.postMessage(bytes); }",
    ),
    finding: /transfers 1 port/,
  },
  // A listed shape, once too often: the count is what refuses it.
  {
    name: "a listed message once too often",
    ...before("self.postMessage({ starting: true });"),
    finding: /one starting too many/,
  },
  // THE SETTLE'S TWO CLOCKS, each witnessed alone. A copy 700 ms after the plant lands after the
  // worker's 500 ms echo and before the page's own 500 ms wait ends: so the plain one is caught
  // only because the page waits too, and the one behind a shortened `setTimeout` -- which would
  // bring the echo forward to ~0 -- only because the settle captured its timer before the bundle
  // ran. Each was green with its own defence removed and the other in place.
  {
    name: "a copy 700 ms later",
    ...before("setTimeout(() => self.postMessage({ copy: bytes }), 700);"),
    finding: BYTES,
  },
  {
    name: "a copy 700 ms later, behind a shortened setTimeout",
    ...before(
      "{ const t = self.setTimeout; self.setTimeout = (f, ms, ...a) => t(f, 0, ...a); t(() => self.postMessage({ copy: bytes }), 700); }",
    ),
    finding: BYTES,
  },
  {
    // In the SHARED protocol, which `mutate` reaches because it acts on the whole bundle's text.
    name: "a second copy of the document, 200 ms after the reply",
    from: "    self.postMessage(drainReply(request.id, reply));",
    to: "    const d = drainReply(request.id, reply); self.postMessage(d); setTimeout(() => self.postMessage({ copy: d.output }), 200);",
    finding: BYTES,
  },
  {
    name: "a reply carrying two documents",
    from: "      output:\n",
    to: "      extra: new Blob([new Uint8Array(4)]),\n      output:\n",
    finding: /carries 2 byte values; exactly one may/,
  },
  {
    // Content in a listed field of the reply itself, which only the length bound can see.
    name: "a reply whose rotations are 300 long",
    from: "      rotations: Array.from(reply.rotations, (n) => Number(n)),\n",
    to: "      rotations: Array.from({ length: 300 }, () => 0),\n",
    finding: /holds a run of 300/,
  },
];

for (const leak of LEAKS) {
  test(`SHOWN TO FAIL: a copy of the worker that posts ${leak.name} is caught`, async ({
    page,
  }) => {
    await openHarness(page);
    const { applied } = await page.evaluate(
      (mutate) => window.burrowHarness.armRedaction({ mutate }),
      { from: leak.from, to: leak.to },
    );
    // THE MUTATION APPLIED, or this case measures the real worker and proves nothing.
    expect(applied, `the planted leak did not apply: ${leak.name}`).toBe(true);

    const { log, nonce } = await redactAndSettle(page, 1);
    expect(
      r8Violations(log, onlyRequest(log), nonce).join("\n"),
      "R8's check passed a leaking worker",
    ).toMatch(leak.finding);
  });
}

/**
 * THE RULES NO COPY OF THIS WORKER REACHES, on hand-written logs. Each changes one thing in
 * `cleanLog`, and `cleanLog` itself is the near-miss beside all of them.
 */
const HAND: {
  name: string;
  change: (log: RedactionMessage[]) => RedactionMessage[];
  finding: RegExp;
}[] = [
  {
    name: "a log read before the worker went quiet",
    change: (log) => log.slice(0, 2),
    finding: /before the worker went quiet/,
  },
  { name: "no terminal reply", change: (log) => [log[0], log[2]], finding: /no terminal reply/ },
  {
    name: "a second reply under another id",
    change: (log) => [log[0], { ...log[1], id: 8 }, ...log.slice(1)],
    finding: /is a second reply/,
  },
  {
    name: "a reply of an unlisted shape",
    change: (log) => [log[0], { ...log[1], shape: "{id:number,ok:true,output:bytes}" }, log[2]],
    finding: /terminal reply has a shape nobody listed/,
  },
  {
    name: "a stub's report naming no exit anyone listed",
    change: (log) => [
      ...log,
      {
        ...log[2],
        keys: ["__burrowSideChannel"],
        settled: null,
        sideChannel: "nowhere",
        shape: "{__burrowSideChannel:string}",
      },
    ],
    finding: /has a shape nobody listed/,
  },
  {
    name: "a reply that transfers a port",
    change: (log) => [log[0], { ...log[1], ports: 1 }, log[2]],
    finding: /transfers 1 port/,
  },
  {
    name: "a successful reply carrying nothing",
    change: (log) => [log[0], { ...log[1], bytes: 0 }, log[2]],
    finding: /carries 0 byte values; exactly one may/,
  },
  {
    name: "a refusal carrying bytes",
    change: (log) => [log[0], { ...log[1], ok: false }, log[2]],
    finding: /a refusal, and it carried 1 byte value/,
  },
];

test("the hand-written near-miss passes every rule", () => {
  expect(r8Violations(cleanLog(7, 42), 7, 42)).toEqual([]);
});

for (const hand of HAND) {
  test(`SHOWN TO FAIL, by hand: ${hand.name}`, () => {
    expect(r8Violations(hand.change(cleanLog(7, 42)), 7, 42).join("\n")).toMatch(hand.finding);
  });
}
