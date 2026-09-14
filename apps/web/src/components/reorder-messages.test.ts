import { describe, expect, it } from "vitest";

import { type Failure, messageFor } from "./reorder-messages.js";

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

describe("what reorder says differently from rotate", () => {
  // The two modules are deliberately separate and mostly identical. What is NOT identical is
  // asserted here, so a later edit that quietly makes them the same fails rather than passing.
  it("says the document was not reordered, not that it was not changed", () => {
    const message = messageFor({
      kind: "LimitExceeded",
      limit: "max_pages",
      requested: "20000",
      allowed: "10000",
    });
    expect(message.next).toContain("not reordered");
  });

  it("says the file on your computer is untouched when a run is stopped on time", () => {
    // A person whose reorder was cut off mid-way wants to know their original is intact --
    // more than for a rotation, because "the pages were being moved" sounds like the file was
    // being edited in place. It never is: the input is read and a new document is produced.
    const message = messageFor({ kind: "LimitExceeded", limit: "max_duration_ms" });
    expect(message.next).toContain("untouched");
  });
});

describe("a reorder failure, as a sentence", () => {
  it("has a message for every kind the core can produce", () => {
    // GATED ON THE COUNT, not merely on each one being non-empty: a kind removed from this
    // list would take its assertion with it and the suite would still pass.
    expect(KINDS).toHaveLength(9);
    for (const kind of KINDS) {
      const message = messageFor({ kind });
      expect(message.title, `${kind} has no title`).not.toBe("");
      expect(message.title.endsWith("."), `${kind}'s title is not a sentence`).toBe(true);
    }
  });

  it("never apologises, and never says an error occurred", () => {
    // The design brief's writing rules, asserted rather than trusted. "Sorry, something went
    // wrong" is the shape this file exists to prevent.
    for (const kind of KINDS) {
      const { title, next } = messageFor({ kind });
      const text = `${title} ${next}`.toLowerCase();
      for (const banned of ["sorry", "oops", "unexpected error", "an error occurred", "please"]) {
        expect(text, `${kind} says "${banned}"`).not.toContain(banned);
      }
    }
  });

  it("says something of its own, rather than falling through to the fallback", () => {
    // THE NEAR-MISS THIS SUITE WAS MISSING. Code review measured it: with the `Unsupported`
    // branch deleted, all 11 tests passed; with `PasswordRequired` deleted, all 11 passed.
    // The loops above only require a sentence of some length ending in a full stop, and the
    // fallback satisfies that -- a rule that matches everything, which is not a rule. It is
    // the same defect a review caught in `merge-messages.test.ts`, repeated by copying the
    // weaker half of that file and not the stronger.
    //
    // Two kinds SHARE the fallback deliberately, and they are named rather than excluded by a
    // predicate, so adding a kind means deciding which side it is on: `Io` and `Internal` are
    // not about the person's file and there is nothing different to say about them.
    const fallback = messageFor({ kind: "Internal" }).title;
    const sharesTheFallback = ["Io", "Internal"];

    for (const kind of KINDS.filter((k) => !sharesTheFallback.includes(k))) {
      expect(
        messageFor({ kind }).title,
        `${kind} fell through to the fallback, so its branch is doing nothing`,
      ).not.toBe(fallback);
    }
    for (const kind of sharesTheFallback) {
      expect(messageFor({ kind }).title, `${kind} is expected to share the fallback`).toBe(
        fallback,
      );
    }

    // And they are distinct from EACH OTHER, not merely from the fallback: two branches
    // returning the same sentence would pass every assertion above.
    const distinct = new Set(KINDS.map((k) => messageFor({ kind: k }).title));
    expect(
      distinct.size,
      `${KINDS.length} kinds, ${sharesTheFallback.length} of them sharing one sentence`,
    ).toBe(KINDS.length - sharesTheFallback.length + 1);
  });

  it("says what to do next for everything a person can act on", () => {
    for (const kind of KINDS) {
      const message = messageFor({ kind });
      expect(message.next, `${kind} says nothing about what to do`).not.toBe("");
    }
  });

  it("marks only EngineUnavailable as needing a deliberate gesture", () => {
    // The breaker latches on purpose (ADR 0015 §3). Everything else can be retried by acting
    // on the page, and a tool that offered "try again" for a latched breaker would be lying.
    for (const kind of KINDS) {
      expect(messageFor({ kind }).retryable, `${kind}`).toBe(kind !== "EngineUnavailable");
    }
  });

  it("gives both numbers for a size ceiling", () => {
    // "Too large" without a size is a refusal a person cannot act on.
    const message = messageFor({
      kind: "LimitExceeded",
      limit: "max_input_bytes",
      requested: String(600 * 1024 * 1024),
      allowed: String(512 * 1024 * 1024),
    });
    expect(message.title).toContain("600 MB");
    expect(message.title).toContain("512 MB");
  });

  it("gives both numbers for a page ceiling, with thousands separated", () => {
    const message = messageFor({
      kind: "LimitExceeded",
      limit: "max_pages",
      requested: "12000",
      allowed: "10000",
    });
    expect(message.title).toContain("12,000");
    expect(message.title).toContain("10,000");
  });

  it("still says something usable when a ceiling arrives with no numbers", () => {
    // Reachable: `max_memory_bytes` at the `measured` stage omits `requested` on purpose,
    // because the two platforms measure different things (ADR 0007).
    for (const limit of ["max_input_bytes", "max_pages", "max_duration_ms", "max_memory_bytes"]) {
      const message = messageFor({ kind: "LimitExceeded", limit });
      expect(message.title, limit).not.toBe("");
      expect(message.title, limit).not.toContain("undefined");
      expect(message.title, limit).not.toContain("NaN");
    }
  });

  it("does not claim memory was prevented, because it was only detected", () => {
    // ADR 0007: `max_memory_bytes` detects, never bounds. A sentence saying burrow "stopped
    // it from" using the memory would claim a guarantee that does not exist -- the memory was
    // already spent when this was noticed, and the message says so.
    const message = messageFor({ kind: "LimitExceeded", limit: "max_memory_bytes" });
    expect(`${message.title} ${message.next}`).toContain("already been read");
  });

  it("blames the page, not the file, for an invalid argument", () => {
    // The selection box catches a bad page number before the core sees it, so arriving here
    // means the page and the core disagree. Telling somebody their file is broken would send
    // them looking in the wrong place.
    const message = messageFor({ kind: "InvalidArgument" });
    expect(message.next).toContain("bug in the page");

    // AND IT IS ABOUT AN ORDER, not a generic bad request. These tests began as a copy of
    // `rotate-messages.test.ts`, and a copy that asserts only what both files share is a copy
    // that measures nothing about the differences -- which are the whole reason this is a
    // second module. `resolveOrder` refuses a non-permutation with the page number before
    // anything is posted, so reaching this branch means the page and the core disagree.
    expect(message.title).toContain("order");
    expect(message.next).toContain("exactly once");
  });

  it("tells a person their file is intact when burrow itself failed", () => {
    for (const kind of ["Io", "Internal"]) {
      expect(messageFor({ kind }).next).toContain("nothing was sent anywhere");
    }
  });

  it("handles a variant it has never heard of", () => {
    // `burrow_types::Error` is `#[non_exhaustive]`. A new variant must arrive as something a
    // person can act on rather than as a blank.
    const message = messageFor({ kind: "SomethingAddedUpstream" } as Failure);
    expect(message.title).not.toBe("");
    expect(message.next).not.toBe("");
  });
});
