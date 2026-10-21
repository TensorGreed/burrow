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
// - A message later than `2 * SETTLE_MS` after the settle handshake, on the page's clock.
// - NUMBER-SHAPED data in fields whose values are bounded but not fixed -- some sixty bytes a
//   reply, and about 650 more in a success's report at its 64-font ceiling (`FIELD_RULES` has
//   the arithmetic). Not text, but not nothing.
// - What a REFUSAL's `message` says: Rust's prose, and an error quoting the document would pass,
//   at any length. The typed-error rule holds that, not this spec.
// - A `MessagePort` the page never received. One transferred to the page is caught; #206's
//   second-engine reading will add a worker-to-worker port, and ADR 0029's 2026-09-27 amendment
//   requires this spec to follow it.

import { expect, test } from "@playwright/test";

import type { RedactionMessage } from "../src/host/harness-api";
import { openHarness } from "./harness";
import {
  FIELD_RULES,
  REFINED,
  REFUSED_BY,
  THE_CALL,
  WRITER,
  WRITER_FRAMES,
  cleanFramesLog,
  cleanLog,
  framesAndSettle,
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
  // R10'S DIGEST RODE WITH IT, and is the recorder's own SHA-256 of the output it rode with: the
  // positive control for `outputDigestConsistency`, which a reply carrying no digest would skip.
  const terminal = log.filter((m) => m.sent === null && m.ok === true);
  expect(terminal, "the recorder never saw the reply").toHaveLength(1);
  expect(terminal[0].fields["output.sha256"], "the recorder took no digest").toMatch(
    /^[0-9a-f]{64}$/,
  );
  expect(terminal[0].fields.outputDigest).toBe(terminal[0].fields["output.sha256"]);
  expect(reply.outputSha256, "the harness's own digest of the same Blob").toBe(
    terminal[0].fields.outputDigest,
  );
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

// #136's read-only op answers through the same worker, the same protocol and the same rules: its
// reply is listed, carries no bytes, and holds only sizes and rule names.
test("page frames post no bytes, and their reply is a listed shape", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({}));
  const { reply, log, nonce } = await framesAndSettle(page, WRITER);
  expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(true);
  // THE RECORDER SAW THE FRAMES: one that recorded nothing would pass every rule below.
  const terminal = log.filter((m) => m.sent === null && m.ok === true);
  expect(terminal, "the recorder never saw the reply").toHaveLength(1);
  expect(terminal[0].fields["frames[]"], "no frames were recorded").toMatch(/^\d/);
  // EXACTLY THE PINNED READING: the general rule cannot know the true page count, the fixture can.
  expect(WRITER_FRAMES.frames.length, "the golden file has no page for WRITER").toBeGreaterThan(0);
  expect(terminal[0].fields.pages).toBe(WRITER_FRAMES.frameRefusals.length);
  expect(String(terminal[0].fields["frames[]"]).split(",").map(Number)).toEqual(
    WRITER_FRAMES.frames,
  );
  expect(String(terminal[0].fields["frameRefusals[]"]).split(",")).toEqual(
    WRITER_FRAMES.frameRefusals,
  );
  expect(log.some((m) => m.sent === null && m.bytes > 0)).toBe(false);
  expect(r8Violations(log, onlyRequest(log, "page_frames"), nonce)).toEqual([]);
});

test("refused page frames post no bytes either", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({}));
  const { reply, log, nonce } = await framesAndSettle(page, new TextEncoder().encode("not a pdf"));
  expect(reply.ok).toBe(false);
  expect(r8Violations(log, onlyRequest(log, "page_frames"), nonce)).toEqual([]);
});

/** Leaks a copy of the worker plants on the page-frames path, each the finding R8 must report. */
const FRAME_CALL = "    return wasm_bindgen.page_frames(";
const FRAME_LEAKS: { name: string; from: string; to: string; finding: RegExp }[] = [
  {
    name: "a chunk before the frames are read",
    from: FRAME_CALL,
    to: `    self.postMessage({ id: request.id + 1000, chunk: bytes.subarray(0, 16) });\n${FRAME_CALL}`,
    finding: /has a shape nobody listed/,
  },
  {
    name: "a number past any page's size among the frames",
    from: "      frames: reply.frames === undefined ? [] : Array.from(reply.frames),\n",
    to: "      frames: reply.ok ? [612, 4111111111111111] : [],\n",
    finding: /a value no rule admits in frames\[\]/,
  },
  {
    name: "text among the rule names",
    from: "      frameRefusals: reply.frameRefusals === undefined ? [] : Array.from(reply.frameRefusals),\n",
    to: '      frameRefusals: reply.ok ? ["123-45-6789"] : [],\n',
    finding: /a value no rule admits in frameRefusals\[\]/,
  },
];

// THE DOCUMENT AS SMALL NUMBERS: a copy whose frames reply carries the input's first 32 bytes as
// sixteen size pairs, with sixteen matching "" rules -- every item a size or a rule name the
// per-field rules admit. Only the counts against `pages` catch it (`framesConsistency`).
FRAME_LEAKS.push({
  name: "the document as small numbers with matching refusals",
  from: FRAME_CALL,
  to:
    "    { const r = wasm_bindgen.page_frames(bytes, password, webLimits(request.limits));" +
    " const leak = Array.from(bytes.subarray(0, 32), (b) => b + 1);" +
    " const o = {}; const p = Object.getPrototypeOf(r);" +
    " for (const k of Object.getOwnPropertyNames(p)) { const d = Object.getOwnPropertyDescriptor(p, k); if (d && d.get) o[k] = r[k]; }" +
    " o.frames = Float64Array.from(leak); o.frameRefusals = leak.filter((_, i) => i % 2 === 0).map(() => '');" +
    " o.takeOutput = () => r.takeOutput(); o.free = () => r.free(); return o; }\n" +
    FRAME_CALL,
  finding: /carries 32 sizes and 16 rule names for 1 page/,
});

// ONE PAGE, MANY NAMES: a copy whose one-page reply joins several rule names into its single
// refusal. Every part is a rule name and the counts hold, so only splitting a one-page reply
// as any other catches it (round 2 of S2's security review).
FRAME_LEAKS.push({
  name: "rule names joined into a one-page refusal",
  from:
    "      frames: reply.frames === undefined ? [] : Array.from(reply.frames),\n" +
    "      frameRefusals: reply.frameRefusals === undefined ? [] : Array.from(reply.frameRefusals),\n",
  to:
    "      frames: reply.ok ? [0, 0] : [],\n" +
    '      frameRefusals: reply.ok ? ["rotate,user-unit,display-box"] : [],\n',
  finding: /rule names do not split into 1/,
});

/** The frames rules no copy of the worker reaches, on hand-written logs (#136). */
const FRAME_HAND: {
  name: string;
  change: (fields: Record<string, unknown>) => Record<string, unknown>;
  finding: RegExp;
}[] = [
  {
    name: "one size pair too many",
    change: (f) => ({ ...f, "frames[]": "612,792,0,0,1,1", "frames[].length": 6 }),
    finding: /carries 6 sizes and 2 rule names for 2 page/,
  },
  {
    name: "one rule name too few",
    change: (f) => ({ ...f, "frameRefusals[]": "", "frameRefusals[].length": 1 }),
    finding: /carries 4 sizes and 1 rule names for 2 page/,
  },
  {
    name: "more pages than the request allowed",
    change: (f) => ({ ...f, pages: 10001 }),
    finding: /pages 10001 is not from 1 to the request's maxPages 10000/,
  },
  {
    name: "no pages",
    change: (f) => ({ ...f, pages: 0 }),
    finding: /pages 0 is not from 1/,
  },
  {
    name: "a frame beside a rule",
    change: (f) => ({ ...f, "frames[]": "612,792,1,1" }),
    finding: /page 1 has a frame and a rule/,
  },
  {
    name: "no frame and no rule",
    change: (f) => ({ ...f, "frameRefusals[]": "," }),
    finding: /page 1 has no frame and no rule/,
  },
  {
    // R10: a digest with no document beside it binds nothing, and is 32 bytes of channel.
    name: "an outputDigest beside no output",
    change: (f) => ({ ...f, outputDigest: "a".repeat(64) }),
    finding: /a reply with no output carries an outputDigest/,
  },
];

test("the hand-written frames reply passes every rule", () => {
  expect(r8Violations(cleanFramesLog(7, 42), 7, 42)).toEqual([]);
});

for (const hand of FRAME_HAND) {
  test(`SHOWN TO FAIL, by hand: a frames reply with ${hand.name}`, () => {
    const log = cleanFramesLog(7, 42);
    const changed = [log[0], { ...log[1], fields: hand.change(log[1].fields) }, log[2]];
    expect(r8Violations(changed as typeof log, 7, 42).join("\n")).toMatch(hand.finding);
  });
}

for (const leak of FRAME_LEAKS) {
  test(`SHOWN TO FAIL: a copy of the worker whose page frames post ${leak.name} is caught`, async ({
    page,
  }) => {
    await openHarness(page);
    const { applied } = await page.evaluate(
      (mutate) => window.burrowHarness.armRedaction({ mutate }),
      { from: leak.from, to: leak.to },
    );
    expect(applied, `the planted leak did not apply: ${leak.name}`).toBe(true);
    const { log, nonce } = await framesAndSettle(page, WRITER);
    expect(
      r8Violations(log, onlyRequest(log, "page_frames"), nonce).join("\n"),
      "R8's check passed a leaking worker",
    ).toMatch(leak.finding);
  });
}

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

/** Page 1 succeeds; a leak with `page: 2` runs on a refusal (one past the end). */
const LEAKS: { name: string; from: string; to: string; finding: RegExp; page?: number }[] = [
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
  // The output's own text: a File's name, and a Blob's type.
  {
    // Text, not the input: the channel is what is being shown, and a name is text of any length.
    name: "text as the output File's name",
    ...afterReply(
      'self.postMessage({ ...d, output: new File([new Uint8Array(4)], "x".repeat(300)) });',
    ),
    finding: REPLY_UNLISTED,
  },
  {
    name: "a secret as the output's type",
    from: '        reply.outputLength > 0 ? new Blob([reply.takeOutput()], { type: "application/pdf" }) : null,\n',
    to: '        reply.outputLength > 0 ? new Blob([reply.takeOutput()], { type: "text/x-123-45-6789" }) : null,\n',
    finding: /a value no rule admits in output.type/,
  },
  {
    // At the TOP of the bundle, before anything of its own: the prologue's list is already first.
    name: "a forged armed list before the bundle's first message, vouching for reports",
    from: "  self.postMessage({ starting: true });",
    // ONCE: this line runs per engine module, and a second forged list is caught at index 1
    // whether or not the prologue posted one -- which left the always-posted list unwitnessed.
    to: '  if (!self.__forged) { self.__forged = true; self.postMessage({ __burrowSideChannelArmed: ["locks"] }); self.postMessage({ __burrowSideChannel: "locks" }); } self.postMessage({ starting: true });',
    finding: UNLISTED,
  },
  // A stub's report with no stub armed: no exit was watched, so there is nothing to report.
  {
    name: "a forged stub report",
    ...before('self.postMessage({ __burrowSideChannel: "locks" });'),
    finding: UNLISTED,
  },
  // A short secret in a field whose TYPE is right: only the value rules see it.
  {
    // A refusal has no report; the grammar would admit hundreds of bytes of numbers in one.
    name: "a report on a refusal",
    ...field(
      "report: reply.report,",
      'report: reply.ok ? reply.report : "Report { fonts: [], dropped_carried_text: 7 }",',
    ),
    finding: /a value no rule admits in refusalReport/,
    page: 2,
  },
  {
    // A success has no message; a refusal's is the one field left unchecked.
    name: "a secret in a success's message",
    ...field("message: reply.message,", 'message: reply.ok ? "123-45-6789" : reply.message,'),
    finding: /a value no rule admits in message/,
  },
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
  // R10's digest: well-formed, so only the comparison with the output's own bytes sees it.
  {
    name: "a well-formed outputDigest that is not the output's",
    ...field(
      "...(reply.outputDigest === undefined ? {} : { outputDigest: reply.outputDigest }),",
      'outputDigest: "0".repeat(64),',
    ),
    finding: /the reply's outputDigest is not the SHA-256 of its output/,
  },
  {
    name: "an outputDigest on a refusal",
    ...field(
      "...(reply.outputDigest === undefined ? {} : { outputDigest: reply.outputDigest }),",
      'outputDigest: reply.ok ? reply.outputDigest : "0".repeat(64),',
    ),
    finding: /a reply with no output carries an outputDigest/,
    page: 2,
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

    const { log, nonce } = await redactAndSettle(page, leak.page ?? 1);
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
    name: "an outputDigest that is not its output's SHA-256",
    change: (log) => [
      log[0],
      { ...log[1], fields: { ...log[1].fields, outputDigest: "b".repeat(64) } },
      log[2],
    ],
    finding: /the reply's outputDigest is not the SHA-256 of its output/,
  },
  {
    // The witness: with no recorder digest the comparison would pass anything.
    name: "an output the recorder took no digest of",
    change: (log) => {
      const { "output.sha256": _taken, ...fields } = log[1].fields;
      return [log[0], { ...log[1], fields }, log[2]];
    },
    finding: /the recorder took no digest of the reply's output/,
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
    name: "an armed list naming an exit twice",
    change: (log) => [log[0], armed(["Worker", "Worker"]), ...log.slice(1)],
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

/** The finding that names `key` among the fields no rule admits. Keys carry `.` and `[]`. */
const namesField = (key: string) =>
  new RegExp(
    `a value no rule admits in ([\\w.\\[\\]]+,)*${key.replace(/[.[\]]/g, "\\$&")}(,|$)`,
    "m",
  );

// ONE CASE PER FIELD RULE: a validator shared by several fields was witnessed once and left every
// other field using it unwitnessed, so loosening `requested` to any string stayed green (review of
// #137). Each case puts that rule's refused value in the clean reply, and must name that field.
for (const key of Object.keys(FIELD_RULES)) {
  test(`SHOWN TO FAIL, by hand: a value the ${key} rule refuses`, () => {
    const log = cleanLog(7, 42);
    const changed = [
      log[0],
      { ...log[1], fields: { ...log[1].fields, [key]: REFUSED_BY[key] } },
      log[2],
    ];
    expect(r8Violations(changed, 7, 42).join("\n")).toMatch(namesField(key));
  });
}

// THE ARMING ITSELF: a mutation that matched nowhere, or in two places, is reported as not applied
// -- or every "SHOWN TO FAIL" above could be measuring the real worker.
for (const [name, from] of [
  ["a text found nowhere", "this text is in no bundle"],
  ["a text found twice", "self.postMessage("],
] as const) {
  test(`a mutation of ${name} is reported as not applied`, async ({ page }) => {
    await openHarness(page);
    const { applied } = await page.evaluate(
      (mutate) => window.burrowHarness.armRedaction({ mutate }),
      { from, to: "/* planted */" },
    );
    expect(applied).toBe(false);
  });
}

// EACH TIGHTENING, with a value only the tightening refuses.
for (const [key, value, what] of REFINED) {
  test(`SHOWN TO FAIL, by hand: ${key} as ${what}`, () => {
    const log = cleanLog(7, 42);
    const changed = [log[0], { ...log[1], fields: { ...log[1].fields, [key]: value } }, log[2]];
    expect(r8Violations(changed, 7, 42).join("\n")).toMatch(namesField(key));
  });
}
