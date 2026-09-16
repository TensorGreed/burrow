import { describe, expect, it } from "vitest";

import { type Failure, messageFor } from "./split-messages.js";

/** Every kind the core can hand this page, so the switch cannot quietly lose one. */
const KINDS = [
  "PasswordRequired",
  "Malformed",
  "Unsupported",
  "LimitExceeded",
  "InvalidArgument",
  "EngineUnavailable",
  "OutputRejected",
  "Io",
  "Internal",
];

describe("what split says differently from the other three", () => {
  // These modules are deliberately separate and mostly identical. What is NOT identical is
  // asserted here, so a later edit that quietly makes them the same fails rather than passing.

  it("gives the REASON layers are refused, not just the fact", () => {
    // ADR 0019 §4, which is explicit that the reason is the load-bearing part: "a page that
    // said only 'documents with layers cannot be split' would read as a bug". The page prose
    // carries that reason, and a refusal that shrugged would contradict it on the one screen
    // where somebody is actually blocked.
    const message = messageFor({ kind: "Unsupported" });
    expect(message.next).toContain("layers");
    expect(message.next).toContain("recorded for the document as a whole");
    expect(message.next).toContain("hidden");
  });

  it("does not assert that layers are the ONLY cause, because the page cannot see that", () => {
    // `Unsupported` is not reserved for optional content. Today it is the only thing split
    // produces it for, and "usually" is the honest width of that claim.
    expect(messageFor({ kind: "Unsupported" }).next).toContain("usually");
  });

  it("says the document was not split, not that it was not reordered", () => {
    const message = messageFor({
      kind: "LimitExceeded",
      limit: "max_pages",
      requested: "20000",
      allowed: "10000",
    });
    expect(message.next).toContain("not split");
  });

  it("says no PARTS were handed over when a run is stopped on time", () => {
    // Split is checkpointed BETWEEN parts (ADR 0023 §6), so a timeout can land with some
    // parts already produced -- and none delivered (§3). "Nothing was changed" is true and
    // is not the whole answer: a person wants to know they are not holding half a document.
    const message = messageFor({ kind: "LimitExceeded", limit: "max_duration_ms" });
    expect(message.next).toContain("No parts were handed over");
    expect(message.next).toContain("untouched");
  });

  it("names a PART in the output refusal, because there is more than one document", () => {
    // ADR 0022 + ADR 0023 §3. "burrow checked the document" would be the wrong number of
    // documents, and the all-or-nothing rule is the thing a person needs told.
    const message = messageFor({ kind: "OutputRejected" });
    expect(message.title).toContain("parts");
    expect(message.next).toContain("One part");
    expect(message.next).toContain("refused the whole split");
  });

  it("does not tell somebody their file is fine when burrow refused its own output", () => {
    // The generic fallback says "Your file is fine … Try again". Both halves are false for
    // OutputRejected, which is the defect `error-kinds.test.ts` was written after.
    const message = messageFor({ kind: "OutputRejected" });
    expect(message.title).not.toContain("Something inside Not Only PDF failed");
    // THE POSITIVE HALF. Without it the line above is satisfied by a sentence that no longer
    // exists anywhere -- which is what a product rename did to four assertions in this
    // repository at once. Asserting the fallback is still worded this way is what keeps the
    // negative meaning something.
    expect(messageFor({ kind: "Internal" }).title).toBe("Something inside Not Only PDF failed.");
    expect(message.next).not.toContain("Your file is fine");
  });

  it("says no parts were handed over even when burrow itself failed", () => {
    // The Internal fallback on the other three pages ends "Try again"; here a person may be
    // looking at a list of download links that is about to vanish, so it says what they hold.
    expect(messageFor({ kind: "Internal" }).next).toContain("No parts were handed over");
  });
});

describe("a split failure, as a sentence", () => {
  it("has a message for every kind the core can produce", () => {
    // GATED ON THE COUNT, not merely on each one being non-empty: a kind removed from this
    // list would take its assertion with it and the suite would still pass.
    expect(KINDS).toHaveLength(9);
    for (const kind of KINDS) {
      const message = messageFor({ kind });
      expect(message.title, kind).not.toBe("");
      expect(message.next, kind).not.toBe("");
    }
  });

  it("gives every kind but one a way back without a deliberate gesture", () => {
    // `retryable: false` renders the Start again button, which clears the circuit breaker
    // (ADR 0015 §3). Offering it when the breaker has not latched would be the page lying
    // about its own state.
    for (const kind of KINDS) {
      expect(messageFor({ kind }).retryable, kind).toBe(kind !== "EngineUnavailable");
    }
  });

  it("names both numbers when the core gave them", () => {
    // "Too large" without a size is a refusal a person cannot act on: they do not know
    // whether to remove one page or ninety.
    const big: Failure = {
      kind: "LimitExceeded",
      limit: "max_input_bytes",
      requested: String(600 * 1024 * 1024),
      allowed: String(512 * 1024 * 1024),
    };
    expect(messageFor(big).title).toContain("600 MB");
    expect(messageFor(big).title).toContain("512 MB");
  });

  it("falls back to a sentence without numbers when the core gave none", () => {
    // A missing number must not render as "That file is , and burrow stops at .".
    const message = messageFor({ kind: "LimitExceeded", limit: "max_input_bytes" });
    expect(message.title).toBe("That file is larger than Not Only PDF will open.");
  });

  it("does not claim memory was bounded, because it was not", () => {
    // ADR 0007: `max_memory_bytes` is DETECTED, not prevented. "burrow stopped it" would
    // claim a ceiling that does not exist -- the memory was already spent.
    const message = messageFor({ kind: "LimitExceeded", limit: "max_memory_bytes" });
    expect(message.next).toContain("already been read");
  });

  it("blames the page, not the file, when the cuts were refused by the core", () => {
    // `resolveCuts` refuses every bad cut list beside the box. Arriving here means the page
    // and the core disagree about the document, which is a bug in the page.
    expect(messageFor({ kind: "InvalidArgument" }).next).toContain("bug in the page");
  });

  it("gives an unknown kind something a person can act on", () => {
    // `Error` is `#[non_exhaustive]`, so a variant added upstream must not arrive as a blank.
    const message = messageFor({ kind: "SomethingAddedLater" });
    expect(message.title).not.toBe("");
    expect(message.next).not.toBe("");
  });
});
