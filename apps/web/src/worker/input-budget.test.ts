// The size ceiling is applied BEFORE a single byte is read, and that ordering is the change.
//
// `burrow_core::ops::merge` checks the aggregate `max_input_bytes` before it opens any
// document. That is the right place in the core and it is too late on the web, because the
// transport gets there first: the worker reads every `Blob`, copies them into one flat buffer,
// and wasm-bindgen copies that into linear memory -- roughly four times the payload before
// Rust is consulted. A selection several times over the ceiling can exhaust the tab on the way
// in, and what the person sees is "something inside burrow failed" rather than the refusal the
// ceiling exists to give them. Issue #51.
//
// WHY THIS IS THE TEST FOR IT. The conformance corpus asserts that both implementations
// produce the same typed OUTCOME for an over-ceiling set -- `merge-refuses-when-the-total-
// passes-max-input-bytes` -- and an outcome says nothing about when it was reached. The claim
// here is about order, and order is only observable from inside the worker: did anything read
// the file before deciding not to?
//
// The scope is shared with `init-memoisation.test.ts` (`./test-scope.ts`), which already
// counts blob reads for a different reason.

import { describe, expect, it } from "vitest";

import { MAIN, load, operation, replyShape } from "./test-scope.js";

/** A refusal shaped exactly as `check_input_budget` returns one. */
function overBudget(): Record<string, unknown> {
  return {
    ...replyShape(),
    ok: false,
    kind: "LimitExceeded",
    limit: "max_input_bytes",
    stage: "input_size",
    requested: 600n * 1024n * 1024n,
    allowed: 512n * 1024n * 1024n,
  };
}

/** Drive one operation through a worker whose budget check answers `budget`. */
async function run(budget: () => Record<string, unknown>) {
  const harness = load(MAIN, true, budget);
  harness.releaseQpdf();
  await harness.ensureReady();
  await harness.send(harness.op(1));
  return harness;
}

describe("the input budget is decided before the input is read", () => {
  it("reads nothing when the budget refuses", async () => {
    const harness = await run(overBudget);

    expect(
      harness.blobReads(),
      "the file was read before the ceiling was applied, which is the whole failure this " +
        "check exists to prevent -- the transport allocates about four times the payload " +
        "before Rust is consulted",
    ).toBe(0);
  });

  it("reads the input when the budget allows it", async () => {
    // THE CONTROL. Without it, a worker that had stopped reading files altogether would pass
    // the case above perfectly.
    const harness = await run(() => ({ ...replyShape(), ok: true }));

    expect(harness.blobReads(), "an allowed operation never read its input").toBe(1);
  });

  it("asks in Rust, with the sizes it already has", async () => {
    const harness = await run(() => ({ ...replyShape(), ok: true }));

    expect(
      harness.budgetCalls.length,
      "the worker did not consult the budget at all -- a size comparison in JavaScript would " +
        "be the binding enforcing Limits, which ADR 0009 §2 forbids",
    ).toBe(1);
    // `operation()` hands over one blob whose `size` the scope reports as 8.
    expect(harness.budgetCalls[0]).toEqual([8]);
  });

  it("reports the refusal the core produced, unchanged", async () => {
    const harness = await run(overBudget);
    const reply = harness.posted.find((m) => m["id"] === 1 && m["ok"] === false);

    expect(reply, "no reply was posted for a refused operation").toBeDefined();
    expect(reply?.["kind"]).toBe("LimitExceeded");
    expect(reply?.["limit"]).toBe("max_input_bytes");
    expect(reply?.["stage"]).toBe("input_size");
    // Strings, because these are u64 -- the same shape every other limit failure arrives in.
    expect(reply?.["requested"]).toBe(String(600 * 1024 * 1024));
    expect(reply?.["allowed"]).toBe(String(512 * 1024 * 1024));
    expect(reply?.["fatal"], "a ceiling is an ordinary outcome and must not cost the worker").toBe(
      false,
    );
  });

  it("acks before it refuses, so the watchdog is not left waiting", async () => {
    // The ack is what starts the page's watchdog clock (ADR 0015). A refusal that skipped it
    // would leave the host timing a request the worker had already answered.
    const harness = await run(overBudget);

    const ack = harness.posted.findIndex((m) => m["id"] === 1 && m["ack"] === true);
    const answer = harness.posted.findIndex((m) => m["id"] === 1 && m["ok"] === false);
    expect(ack, "no ack was sent").toBeGreaterThanOrEqual(0);
    expect(answer, "no answer was sent").toBeGreaterThan(ack);
  });

  it("the source really does check before reading, not merely in this stub", () => {
    // A structural cross-check by another route, because everything above runs against a
    // scope this file also wrote. In `main.js` the budget call must appear before the
    // `arrayBuffer()` that reads an input.
    const budgetAt = MAIN.indexOf("check_input_budget");
    const readAt = MAIN.indexOf("await blob.arrayBuffer()");
    expect(budgetAt, "main.js no longer calls check_input_budget").toBeGreaterThan(0);
    expect(readAt, "main.js no longer reads a blob the way this rule looks for").toBeGreaterThan(0);
    expect(
      budgetAt,
      "the budget is consulted after the bytes are read, which is no earlier than the core",
    ).toBeLessThan(readAt);
  });
});
