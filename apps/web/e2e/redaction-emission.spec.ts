// R8's named check (ADR 0006, #137): redaction's output is ONE value, posted only after the Rust
// call returned verified success. Progress may report position; it may not emit content.
//
// Every message redaction's worker posts is recorded by the harness before the host reads it, so
// this sees what the page was handed -- not what the host chose to keep. And it is SHOWN TO FAIL:
// the same predicate is run over a copy of the worker with a chunk posted mid-operation, and the
// mutation is asserted to have applied before its verdict counts.
//
// WHAT IT DOES NOT SEE: a `MessagePort`. Nothing in redaction's worker has one today; #206's
// second-engine reading will, and ADR 0029's 2026-09-27 amendment requires this spec to follow it.

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";
import { THE_CALL, onlyRequest, r8Violations, redactWriter } from "./redaction-protocol";

test("a successful redaction posts its document once, in the terminal reply", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({}));
  const reply = await redactWriter(page, 1);
  expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(true);

  const log = await page.evaluate(() => window.burrowHarness.redactMessages());
  // THE LOG IS NOT EMPTY, and saw the document: a recorder that recorded nothing would pass
  // every "no message carries bytes" rule below.
  expect(
    log.some((m) => m.sent === null && m.bytes > 0),
    "the recorder never saw the document",
  ).toBe(true);
  expect(r8Violations(log, onlyRequest(log))).toEqual([]);
});

test("a refused redaction posts no bytes at all", async ({ page }) => {
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.armRedaction({}));
  const reply = await redactWriter(page, 2); // one past the end
  expect(reply.ok).toBe(false);

  const log = await page.evaluate(() => window.burrowHarness.redactMessages());
  expect(log.length, "the recorder saw nothing").toBeGreaterThan(0);
  expect(r8Violations(log, onlyRequest(log))).toEqual([]);
});

for (const [name, chunk] of [
  ["a chunk naming no request", "self.postMessage({ chunk: bytes.subarray(0, 16) });\n"],
  // The host takes a message with the request's id as its reply (review of #137), so this one
  // does not merely leak -- it becomes the answer. The predicate must see it either way.
  [
    "a chunk under the request's id",
    "self.postMessage({ id: request.id, chunk: bytes.subarray(0, 16) });\n",
  ],
] as const) {
  test(`SHOWN TO FAIL: a copy of the worker that posts ${name} mid-operation is caught`, async ({
    page,
  }) => {
    await openHarness(page);
    const { applied } = await page.evaluate(
      (mutate) => window.burrowHarness.armRedaction({ mutate }),
      { from: THE_CALL, to: `  ${chunk}${THE_CALL}` },
    );
    // THE MUTATION APPLIED, or this case measures the real worker and proves nothing.
    expect(applied, "the planted chunk did not apply to the worker's copy").toBe(true);

    await redactWriter(page, 1);
    const log = await page.evaluate(() => window.burrowHarness.redactMessages());
    const found = r8Violations(log, onlyRequest(log));
    expect(found.join("\n"), "R8's check passed a worker that leaks a chunk").toMatch(
      /carries bytes/,
    );
  });
}

// THE PREDICATE ALONE, on a log written by hand: ADR 0023's "one part, not merely one message".
// No copy of the worker is planted for this one, because a reply carrying two documents cannot be
// made from `redact-main.js` -- `drainReply` in `worker-protocol.js` builds the reply, and every
// bundle shares it byte for byte. What this shows is that the counting refuses it.
test("SHOWN TO FAIL: a single reply carrying two documents is caught", () => {
  const entry = { keys: [], sideChannel: null, armed: null };
  const log = [
    { ...entry, sent: "redact", id: 7, bytes: 1, ok: null },
    { ...entry, sent: null, id: 7, bytes: 2, ok: true },
  ];
  expect(r8Violations(log, 7).join("\n")).toMatch(/carries 2 byte values; exactly one may/);
  // ...and the near-miss beside it passes, so the rule is not simply refusing every reply.
  expect(r8Violations([log[0], { ...log[1], bytes: 1 }], 7)).toEqual([]);
});
