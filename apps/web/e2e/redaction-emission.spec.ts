// R8's named check (ADR 0006, #137): redaction's output is ONE value, posted only after the Rust
// call returned verified success. Progress may report position; it may not emit content.
//
// Every message between the page and redaction's worker is recorded by the harness before the
// host reads it, so this sees what the page's thread was handed -- not what the host chose to
// keep -- and it reads the log only after the worker has gone quiet. It is SHOWN TO FAIL: the
// same predicate is run over copies of the worker that leak, each mutation asserted to have
// applied before its verdict counts.
//
// WHAT IT DOES NOT SEE:
// - WHAT THE REPLY'S OWN STRINGS SAY. `message` and `report` are Rust's; an error quoting the
//   document would pass. The typed-error rule holds that, not this spec.
// - A message deferred by more than `SETTLE_MS`.
// - Content posted from a nested worker or across a `MessagePort` the page never received. R9's
//   stubs report a nested worker's construction, and this spec reports any port transferred to
//   the page; #206's second-engine reading will add a worker-to-worker port, and ADR 0029's
//   2026-09-27 amendment requires this spec to follow it.

import { expect, test } from "@playwright/test";

import type { RedactionMessage } from "../src/host/harness-api";
import { openHarness } from "./harness";
import { THE_CALL, onlyRequest, r8Violations, redactAndSettle } from "./redaction-protocol";

test("a successful redaction posts its document once, in the terminal reply", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({}));
  const { reply, log } = await redactAndSettle(page, 1);
  expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(true);

  // THE RECORDER SAW THE DOCUMENT: one that recorded nothing would pass every "no message carries
  // bytes" rule below.
  expect(
    log.filter((m) => m.sent === null && m.bytes > 0).length,
    "the recorder never saw the document",
  ).toBe(1);
  expect(r8Violations(log, onlyRequest(log))).toEqual([]);
});

test("a refused redaction posts no bytes at all", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({}));
  const { reply, log } = await redactAndSettle(page, 2); // one past the end
  expect(reply.ok).toBe(false);

  expect(
    log.some((m) => m.sent === null && m.ok === false),
    "the recorder never saw the worker's refusal",
  ).toBe(true);
  expect(r8Violations(log, onlyRequest(log))).toEqual([]);
});

/** Each planted leak, where it goes, and the finding R8 must report for it. */
const LEAKS: { name: string; from: string; to: string; finding: RegExp }[] = [
  ...(
    [
      ["a chunk naming no request", "self.postMessage({ chunk: bytes.subarray(0, 16) });"],
      // The host takes a message under the request's id as its reply, so this one does not merely
      // leak -- it becomes the answer, and the real reply arrives after anyone looked.
      [
        "a chunk under the request's id",
        "self.postMessage({ id: request.id, chunk: bytes.subarray(0, 16) });",
      ],
      [
        "a chunk under a foreign id",
        "self.postMessage({ id: request.id + 1000, chunk: bytes.subarray(0, 16) });",
      ],
      [
        "a chunk whose id is undefined",
        "self.postMessage({ id: undefined, chunk: bytes.subarray(0, 16) });",
      ],
    ] as const
  ).map(([name, leak]) => ({
    name,
    from: THE_CALL,
    to: `  ${leak}\n${THE_CALL}`,
    finding: /carries bytes, and is not the terminal reply/,
  })),
  {
    name: "the input as a number array",
    from: THE_CALL,
    to: `  self.postMessage({ progress: Array.from(bytes.subarray(0, 256)) });\n${THE_CALL}`,
    finding: /has a shape nobody listed/,
  },
  {
    // In a shape that IS listed, so only the length rule can see it.
    name: "the input as a string, in a listed shape",
    from: THE_CALL,
    to: `  self.postMessage({ starting: new TextDecoder("latin1").decode(bytes.subarray(0, 256)) });\n${THE_CALL}`,
    finding: /holds a run of 256/,
  },
  {
    name: "the input as ImageData",
    from: THE_CALL,
    to: `  self.postMessage({ starting: new ImageData(new Uint8ClampedArray(bytes.slice(0, 64)), 4, 4) });\n${THE_CALL}`,
    finding: /carries bytes, and is not the terminal reply/,
  },
  {
    name: "a port the input then goes down",
    from: THE_CALL,
    to: `  { const c = new MessageChannel(); self.postMessage({ starting: true }, [c.port2]); c.port1.postMessage(bytes); }\n${THE_CALL}`,
    finding: /transfers 1 port/,
  },
  {
    // In the SHARED protocol, which `mutate` reaches because it acts on the whole bundle's text.
    name: "a second copy of the document, 200 ms after the reply",
    from: "    self.postMessage(drainReply(request.id, reply));",
    to: "    const d = drainReply(request.id, reply); self.postMessage(d); setTimeout(() => self.postMessage({ copy: d.output }), 200);",
    finding: /carries bytes, and is not the terminal reply/,
  },
  {
    name: "a reply carrying two documents",
    from: "      output:\n",
    to: "      extra: new Blob([new Uint8Array(4)]),\n      output:\n",
    finding: /carries 2 byte values; exactly one may/,
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

    const { log } = await redactAndSettle(page, 1);
    expect(
      r8Violations(log, onlyRequest(log)).join("\n"),
      "R8's check passed a leaking worker",
    ).toMatch(leak.finding);
  });
}

// THE SETTLE RULE, on logs written by hand: a log that never saw the worker go quiet is refused,
// and the same log with the marker passes -- so the rule is not simply refusing everything.
test("SHOWN TO FAIL: a log read before the worker went quiet is refused", () => {
  const entry = { keys: [], longest: 0, ports: 0, sideChannel: null, armed: null };
  const log: RedactionMessage[] = [
    { ...entry, sent: "redact", id: 7, bytes: 1, ok: null },
    { ...entry, sent: null, id: 7, keys: ["id", "ok", "output"], bytes: 1, ok: true },
  ];
  expect(r8Violations(log, 7).join("\n")).toMatch(/before the worker went quiet/);
  const settled = { ...entry, sent: null, id: null, keys: ["__burrowSettled"], bytes: 0, ok: null };
  expect(r8Violations([...log, settled], 7)).toEqual([]);
});
