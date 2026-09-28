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
// - A message later than `2 * SETTLE_MS` after the settle handshake, on the page's clock.
// - About ninety bytes a reply of NUMBER-SHAPED data, in fields whose values are bounded but not
//   fixed (see `FIELD_RULES`). Not text, but not nothing.
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
/** A planted leak in the SHARED protocol, right after the reply is posted. */
const REPLY_LINE = "    self.postMessage(drainReply(request.id, reply));";
const afterReply = (leak: string) => ({
  from: REPLY_LINE,
  to: `    const d = drainReply(request.id, reply); self.postMessage(d); ${leak}`,
});
/** A planted change to one field of the reply `drainReply` builds. */
const field = (from: string, to: string) => ({ from: `      ${from}\n`, to: `      ${to}\n` });
const chunk = "bytes.subarray(0, 16)";
const latin1 = 'new TextDecoder("latin1").decode(bytes.subarray(0, 256))';
const UNLISTED = /has a shape nobody listed/;
const REPLY_UNLISTED = /the terminal reply has a shape nobody listed/;

const LEAKS: { name: string; from: string; to: string; finding: RegExp }[] = [
  {
    name: "a chunk naming no request",
    ...before(`self.postMessage({ chunk: ${chunk} });`),
    finding: UNLISTED,
  },
  // The host takes a message under the request's id as its reply, so this one does not merely
  // leak -- it becomes the answer, and the real reply arrives after anyone looked.
  {
    name: "a chunk under the request's id",
    ...before(`self.postMessage({ id: request.id, chunk: ${chunk} });`),
    finding: UNLISTED,
  },
  {
    name: "a chunk under a foreign id",
    ...before(`self.postMessage({ id: request.id + 1000, chunk: ${chunk} });`),
    finding: UNLISTED,
  },
  {
    name: "a chunk whose id is undefined",
    ...before(`self.postMessage({ id: undefined, chunk: ${chunk} });`),
    finding: UNLISTED,
  },
  // Under the keys the harness's own messages use: the exemptions were loose twice.
  {
    name: "a chunk under a __burrow key",
    ...before(`self.postMessage({ __burrow_chunk: ${chunk} });`),
    finding: UNLISTED,
  },
  {
    name: "a chunk as a forged settle echo",
    ...before(`self.postMessage({ __burrowSettled: ${chunk} });`),
    finding: UNLISTED,
  },
  {
    name: "a settle echo with the wrong nonce",
    ...before("self.postMessage({ __burrowSettled: 1 });"),
    finding: UNLISTED,
  },
  {
    name: "a forged armed list",
    ...before('self.postMessage({ __burrowSideChannelArmed: ["createObjectURL"] });'),
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
    finding: UNLISTED,
  },
  {
    name: "the input as a BigInt",
    ...before(
      'self.postMessage({ starting: BigInt("0x" + Array.from(bytes.subarray(0, 64), (b) => b.toString(16).padStart(2, "0")).join("")) });',
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
    finding: UNLISTED,
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
  // Listed shapes, too often, in the wrong combination, or too late: the counts and the position.
  {
    name: "a listed message once too often",
    ...before("self.postMessage({ starting: true });"),
    finding: /one starting too many/,
  },
  {
    name: "both answers to init",
    ...before('self.postMessage({ id: 0, ready: false, fatal: true, kind: "Internal" });'),
    finding: /one init too many/,
  },
  {
    name: "a listed message after the reply",
    ...afterReply("self.postMessage({ id: request.id, ack: true });"),
    finding: /ack message after the reply/,
  },
  // THE SETTLE'S WINDOW, which the page's clock decides: a copy 700 ms after the reply lands
  // after the worker's 500 ms echo and inside the page's 1000 ms from the send. The second copy
  // shortens the worker's timer first, which the page's clock makes irrelevant.
  {
    name: "a second copy, 700 ms after the reply",
    ...afterReply("setTimeout(() => self.postMessage({ copy: d.output }), 700);"),
    finding: UNLISTED,
  },
  {
    name: "a second copy, 700 ms after the reply, behind a shortened setTimeout",
    ...afterReply(
      "{ const t = self.setTimeout; self.setTimeout = (f, ms, ...a) => t(f, 0, ...a); t(() => self.postMessage({ copy: d.output }), 700); }",
    ),
    finding: UNLISTED,
  },
  // The reply itself: its shape, and each field's value.
  {
    name: "a reply carrying two documents",
    ...field("output:", "extra: new Blob([new Uint8Array(4)]),\n      output:"),
    finding: REPLY_UNLISTED,
  },
  {
    name: "a reply whose rotations are not empty",
    ...field("rotations: Array.from(reply.rotations, (n) => Number(n)),", "rotations: [0, 90],"),
    finding: REPLY_UNLISTED,
  },
  {
    // Structured clone carries an array's named properties; a document went past as 426 of them.
    name: "a reply whose rotations carry named properties",
    ...field(
      "rotations: Array.from(reply.rotations, (n) => Number(n)),",
      'rotations: Object.assign([], { a: "x".repeat(300) }),',
    ),
    finding: REPLY_UNLISTED,
  },
  // A short secret in a field whose TYPE is right: only the value rules see it.
  {
    name: "a secret in stage",
    ...field("stage: reply.stage,", 'stage: "123-45-6789",'),
    finding: /a value no rule admits in stage/,
  },
  {
    name: "a secret in allowed",
    ...field("allowed: reply.allowed.toString(),", 'allowed: "4111 1111",'),
    finding: /a value no rule admits in allowed/,
  },
  {
    name: "bits in pages",
    ...field("pages: Number(reply.pages),", "pages: 2 ** 40 + 12345,"),
    finding: /a value no rule admits in pages/,
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
const armed = (names: string[]) => ({
  sent: null,
  id: null,
  keys: ["__burrowSideChannelArmed"],
  bytes: 0,
  fields: {},
  ports: 0,
  ok: null,
  sideChannel: null,
  armed: names,
  settled: null,
  shape: "{__burrowSideChannelArmed:[string]}",
});
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
    finding: REPLY_UNLISTED,
  },
  {
    name: "a reply that transfers a port",
    change: (log) => [log[0], { ...log[1], ports: 1 }, log[2]],
    finding: /transfers 1 port/,
  },
  {
    name: "a kind no Rust arm produces",
    change: (log) => [
      log[0],
      { ...log[1], fields: { ...log[1].fields, kind: "Jane Doe" } },
      log[2],
    ],
    finding: /a value no rule admits in kind/,
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
        fields: {},
        shape: "{__burrowSideChannel:string}",
      },
    ],
    finding: UNLISTED,
  },
  {
    // No listed shape has a field without a rule, so no copy of this worker can reach this one.
    name: "a field no rule is written for",
    change: (log) => [log[0], { ...log[1], fields: { ...log[1].fields, mystery: "x" } }, log[2]],
    finding: /a value no rule admits in mystery/,
  },
  // The armed list: first and once, and naming only exits -- `in` walked the prototype chain.
  {
    name: "an armed list that is not the first message",
    change: (log) => [log[0], log[1], armed(["Worker"]), log[2]],
    finding: UNLISTED,
  },
  {
    name: "an armed list naming a prototype property",
    change: (log) => [log[0], armed(["toString"]), ...log.slice(1)],
    finding: UNLISTED,
  },
];

test("the hand-written near-miss passes every rule, with and without an armed list first", () => {
  const log = cleanLog(7, 42);
  expect(r8Violations(log, 7, 42)).toEqual([]);
  expect(r8Violations([log[0], armed(["Worker", "locks"]), ...log.slice(1)], 7, 42)).toEqual([]);
});

for (const hand of HAND) {
  test(`SHOWN TO FAIL, by hand: ${hand.name}`, () => {
    expect(r8Violations(hand.change(cleanLog(7, 42)), 7, 42).join("\n")).toMatch(hand.finding);
  });
}
