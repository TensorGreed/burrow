// Every typed error has a sentence, and the sentences say something.
//
// A rule-driven check over a fixed set, so it gets what the definition of done asks of one:
// each rule matches its own case, and the near-misses are rejected. The set is small enough
// to name exhaustively, which is what makes "every kind" assertable rather than hopeful.

import { describe, expect, it } from "vitest";

import { messageFor, type Failure } from "./merge-messages.js";

/**
 * Every `kind` a reply can carry.
 *
 * Mirrors `kind_of` in `bindings/burrow-wasm/src/lib.rs`, plus `EngineUnavailable`, which
 * is a HOST verdict rather than an `Error` variant — the page's own willingness to hand out
 * another worker (ADR 0015 §3). A kind added there and not here would fall to the default
 * branch, which is why the list is asserted rather than assumed.
 */
const KINDS = [
  "Malformed",
  "Unsupported",
  "PasswordRequired",
  "LimitExceeded",
  "InvalidArgument",
  "Io",
  "Internal",
  "Unknown",
  "InputFailed",
  "EngineUnavailable",
] as const;

describe("every kind produces something a person can act on", () => {
  it.each(KINDS)("%s says what happened and what to do", (kind) => {
    const message = messageFor({ kind, limit: "max_pages", requested: "20000", allowed: "10000" });

    expect(message.title.length, `${kind}: no title`).toBeGreaterThan(10);
    expect(message.next.length, `${kind}: nothing to do next`).toBeGreaterThan(10);
    expect(message.title.endsWith("."), `${kind}: the title should be a sentence`).toBe(true);
  });

  it.each(KINDS)("%s never apologises or shrugs", (kind) => {
    // The design brief: "errors don't apologize, and they are never vague about what
    // happened". These are the exact phrasings that creep in, so they are named.
    const text = `${messageFor({ kind }).title} ${messageFor({ kind }).next}`.toLowerCase();
    for (const banned of [
      "sorry",
      "oops",
      "unexpected error",
      "something went wrong",
      "please try",
    ]) {
      expect(
        text,
        `${kind}: "${banned}" is the phrasing this rule exists to keep out`,
      ).not.toContain(banned);
    }
  });

  it("says something of its own, rather than falling through to the fallback", () => {
    // THE NEAR-MISS THIS SUITE WAS MISSING, found by code review and measured: with the
    // `Unsupported` branch deleted, all 34 tests passed. `InvalidArgument` too. The loops
    // above only require a sentence of some length ending in a full stop, and the fallback
    // satisfies that -- a rule that matches everything, which is not a rule.
    //
    // Four kinds SHARE the fallback deliberately, and they are named rather than excluded by
    // a predicate, so adding a kind means deciding which side it is on. `Io` and `Internal`
    // are not about the person's files and there is nothing different to say; `Unknown` is
    // the variant-added-upstream case the fallback exists for; and a bare `InputFailed` with
    // no inner kind is a wrapper around nothing, which is the same situation.
    const fallback = messageFor({ kind: "Internal" }).title;
    const sharesTheFallback = ["Io", "Internal", "Unknown", "InputFailed"];

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

  it("names every kind the binding can produce", () => {
    // THE LIST IS THE MEASUREMENT. A `kind` added in Rust and not here would quietly take
    // the default branch, and the suite above would still be green — it would just be
    // testing the fallback nine times.
    expect(new Set(KINDS).size, "duplicate kinds make the count a lie").toBe(KINDS.length);
    expect(KINDS).toContain("Unknown");
  });
});

describe("what a limit failure tells you", () => {
  const limit = (name: string, requested: string, allowed: string): Failure => ({
    kind: "LimitExceeded",
    limit: name,
    requested,
    allowed,
  });

  it("turns a byte ceiling into megabytes", () => {
    const m = messageFor(
      limit("max_input_bytes", String(600 * 1024 * 1024), String(512 * 1024 * 1024)),
    );
    expect(m.title).toContain("600 MB");
    expect(m.title).toContain("512 MB");
    expect(m.next).toContain("two goes");
  });

  it("quotes both page numbers", () => {
    const m = messageFor(limit("max_pages", "20000", "10000"));
    expect(m.title).toContain("20000");
    expect(m.title).toContain("10000");
  });

  it("does not claim the tab ran out of memory", () => {
    // `max_memory_bytes` DETECTS an overrun after the fact (ADR 0007). Saying the browser
    // ran out would be a stronger claim than the code makes, and it would send someone to
    // close other tabs when the file is the problem.
    const m = messageFor(limit("max_memory_bytes", "0", "0"));
    expect(m.title.toLowerCase()).not.toContain("out of memory");
    expect(m.title.toLowerCase()).not.toContain("ran out");
  });

  it("does not blame a file for a ceiling that belongs to the whole set", () => {
    // The core checks the running total as it walks the inputs, so a `max_pages` refusal
    // arrives wrapped as `InputFailed { index }` naming whichever file the total crossed on.
    // That file is fine. Marking it unusable tells someone to remove a good document and
    // then refuses to merge until they do -- found by both reviews, in the exact scenario
    // `e2e/merge-pdf.spec.ts` drives with 74 files.
    for (const name of ["max_pages", "max_input_bytes"]) {
      const m = messageFor({
        kind: "InputFailed",
        innerKind: "LimitExceeded",
        limit: name,
        requested: "10138",
        allowed: "10000",
        failedInput: 72,
      });
      expect(m.file, `${name} is a property of the set, not of input 72`).toBe(-1);
    }
  });

  it("still blames the file for a ceiling that IS about one input", () => {
    // The near-miss for the rule above. If every limit returned -1 the assertion would pass
    // while meaning nothing -- and a file that expands enormously when opened is genuinely
    // the one to remove.
    const m = messageFor({
      kind: "InputFailed",
      innerKind: "LimitExceeded",
      limit: "max_memory_bytes",
      failedInput: 2,
    });
    expect(m.file).toBe(2);
  });

  it("does not blame a file for running out of time", () => {
    // The inversion this project has fixed three times: PR 2 natively, ADR 0015 §2 on the
    // web, and in the operation itself. A deadline is the operation's outcome, so the
    // message must not point at a document — the one it stopped on is probably fine.
    const m = messageFor({ ...limit("max_duration_ms", "0", "0"), failedInput: 2 });
    expect(m.file, "a timeout was attributed to a file").toBe(-1);
  });

  it("still says something useful for a ceiling it does not know", () => {
    // `Limits` can grow a field. The fallback must be a sentence, not a blank.
    const m = messageFor(limit("max_something_new", "9", "8"));
    expect(m.title.length).toBeGreaterThan(10);
    expect(m.next.length).toBeGreaterThan(10);
  });
});

describe("InputFailed is unwrapped, once", () => {
  it("reports what is wrong with the file, not that a wrapper exists", () => {
    const m = messageFor({ kind: "InputFailed", failedInput: 1, innerKind: "PasswordRequired" });
    expect(m.title.toLowerCase()).toContain("password");
    expect(m.title, "the wrapper's name is not something to show a person").not.toContain(
      "InputFailed",
    );
  });

  it("carries the index, so the list can mark the file", () => {
    const m = messageFor({ kind: "InputFailed", failedInput: 2, innerKind: "Malformed" });
    expect(m.file).toBe(2);
  });

  it("does not invent an index when there is none", () => {
    expect(messageFor({ kind: "Malformed" }).file).toBe(-1);
    expect(messageFor({ kind: "InputFailed", failedInput: -1, innerKind: "Malformed" }).file).toBe(
      -1,
    );
  });

  it("survives a wrapper with no inner kind", () => {
    // Defensive: the field is optional in the reply shape, so the component must not
    // depend on the binding always filling it.
    const m = messageFor({ kind: "InputFailed", failedInput: 0 });
    expect(m.title.length).toBeGreaterThan(10);
  });
});

describe("EngineUnavailable is the one a person must clear deliberately", () => {
  it("is not retryable, because the breaker latches on purpose", () => {
    // ADR 0015 §3: retrying on a timer resumes a crash loop at a slower rate rather than
    // ending it. `retryable: false` is what makes the component show a gesture instead of
    // a Merge button.
    const m = messageFor({ kind: "EngineUnavailable" });
    expect(m.retryable).toBe(false);
  });

  it("says nothing is running, because nothing is", () => {
    expect(messageFor({ kind: "EngineUnavailable" }).next.toLowerCase()).toContain("nothing is");
  });

  it("is the only kind that is not retryable", () => {
    // The near-miss for the rule above. If everything were non-retryable the assertion
    // would pass while meaning nothing.
    const notRetryable = KINDS.filter((k) => !messageFor({ kind: k }).retryable);
    expect(notRetryable).toEqual(["EngineUnavailable"]);
  });
});

describe("nothing from a file or an engine can reach a message", () => {
  it("ignores every field it is not given", () => {
    // The input shape carries no free text at all -- there is no field through which an
    // engine's message could arrive. This asserts the property by trying: a reply with
    // hostile-looking extra fields renders identically to one without.
    const plain = messageFor({ kind: "Malformed" });
    const hostile = messageFor({
      kind: "Malformed",
      // @ts-expect-error -- deliberately passing what the type forbids, to prove the
      // function reads only what it declares.
      message: "WARNING: input (offset 4242): xref not found",
      filename: "/home/someone/taxes.pdf",
    });
    expect(hostile).toEqual(plain);
    expect(`${hostile.title} ${hostile.next}`).not.toContain("4242");
    expect(`${hostile.title} ${hostile.next}`).not.toContain("taxes");
  });
});
