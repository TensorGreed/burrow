// The compress tool's sentences, every branch, against planted replies.
//
// # What this suite is really about
//
// The other tools' message tests check that an error reads well. This one checks something
// the page's whole credibility rests on: that a document which did not shrink is presented as
// a FINDING rather than as a failure, and that it explains itself.
//
// Spike 0005 measured a scan at 0.15%, and a scan is the modal thing somebody brings to a page
// called "Compress PDF". If that outcome reads as a broken tool, the tool is worse than not
// shipping it — which is why the three result states have their own assertions here rather
// than being left to whatever the component happens to render.

import { describe, expect, it } from "vitest";

import { NEGLIGIBLE_SAVING, messageFor, percent, resultFor } from "./compress-messages";

/** A reply in the shape `resultFor` reads. Bytes are strings, as `u64` crosses the boundary. */
function reply(original: number, produced: number, hasDocument: boolean) {
  return {
    originalBytes: String(original),
    producedBytes: String(produced),
    hasDocument,
  };
}

const MB = 1024 * 1024;

describe("a real saving", () => {
  it("leads with both sizes and the percentage", () => {
    const r = resultFor(reply(4 * MB, Math.round(2.8 * MB), true));
    expect(r.kind).toBe("smaller");
    expect(r.title).toContain("4.0 MB");
    expect(r.title).toContain("2.8 MB");
    expect(r.title).toContain("30%");
    expect(r.hasDocument).toBe(true);
  });

  it("says nothing was re-encoded, because that is what distinguishes this tool", () => {
    // A person who just watched a file get a third smaller is entitled to know whether their
    // images survived. Tools that shrink a PDF by resampling exist; this is not one.
    const r = resultFor(reply(4 * MB, 2 * MB, true));
    expect(r.detail).toContain("come through unaltered");
    expect(r.detail.toLowerCase()).toContain("no image");
    // AND NOT THE OVERCLAIM. The one lever is object-stream generation and qpdf will flate a
    // content stream that arrived uncompressed, so the decoded instructions are identical and
    // the BYTES are not. `compress_keeps_everything.rs` makes the narrower claim in Rust; this
    // keeps the page from making the wider one.
    expect(r.detail).not.toContain("byte-for-byte");
  });
});

describe("a saving too small to matter — the scan case", () => {
  // 0.15% is what spike 0005 measured on a scanned document, and it is the outcome this page
  // most needs to handle well.
  const scan = resultFor(reply(12 * MB, Math.round(12 * MB * (1 - 0.0015)), true));

  it("is its own state, not dressed up as a success", () => {
    expect(scan.kind).toBe("negligible");
    expect(scan.title).toContain("not worth replacing your file for");
  });

  it("still offers the document, because the page does not decide for the person", () => {
    expect(scan.hasDocument).toBe(true);
  });

  it("explains WHY a scan does not shrink, rather than only reporting a number", () => {
    // THE ASSERTION THIS FILE EXISTS FOR. A bare "0.1%" reads as a broken tool. The reason has
    // to be in the words: the pages are images, the image data is already compressed, and
    // there is no structure left to reorganise.
    expect(scan.detail).toContain("scanned");
    expect(scan.detail).toContain("already compressed");
    expect(scan.detail).toContain("structure");
  });

  it("says burrow will not make it smaller by making it worse", () => {
    // The question a person would otherwise be left with: other tools DO shrink scans, by
    // resampling. Saying why this one does not is the difference between a limitation and a
    // decision.
    expect(scan.detail).toContain("look worse");
    expect(scan.detail).toContain("will not");
  });
});

describe("no saving at all", () => {
  const already = resultFor(reply(Math.round(1.2 * MB), Math.round(1.21 * MB), false));

  it("is a finding about the file, not a failure of the tool", () => {
    expect(already.kind).toBe("unchanged");
    expect(already.title).toBe("This file is already efficiently stored.");
    // NOT an apology, and not a suggestion that anything went wrong.
    expect(already.title.toLowerCase()).not.toContain("could not");
    expect(already.title.toLowerCase()).not.toContain("failed");
    expect(already.title.toLowerCase()).not.toContain("sorry");
  });

  it("reports both numbers, including the one that no longer exists anywhere else", () => {
    // The produced document was discarded, by design -- the core returns two counts rather
    // than a copy of the input (ADR 0025 §3). This string is the only record of its size.
    expect(already.detail).toContain("1.2 MB");
    expect(already.detail).toContain("kept yours");
    expect(already.hasDocument).toBe(false);
  });

  it("carries the same explanation as the negligible case, because the cause is the same", () => {
    const scan = resultFor(reply(12 * MB, Math.round(12 * MB * 0.999), true));
    expect(already.detail).toContain("already compressed");
    expect(scan.detail).toContain("already compressed");
  });
});

describe("the word this page may not use about itself", () => {
  // THE PAGE'S CENTRAL CLAIM IS THAT NOTHING IS RE-ENCODED, and "re-encode" names precisely the
  // lossy thing burrow refuses to do. So no result may report that burrow re-encoded anything.
  // An earlier draft of the `unchanged` branch said "burrow re-encoded it and got ...", which
  // told a person their file had been through exactly the treatment the rest of the page
  // promises it was spared -- on the one outcome where nothing happened at all.
  //
  // The assertion is on the SUBJECT, not on the word: "Nothing was re-encoded" is the sentence
  // this page most wants to say, so a blanket ban on the string would forbid the right sentence
  // along with the wrong one.
  // THE PRODUCT NAME IS IN THE PATTERN, so it moved when the product was renamed. A branch
  // naming a product that no longer appears in any sentence is a branch that can never match:
  // the rule would keep passing while enforcing half of itself, which is the "4 of 15" failure
  // hiding inside a regex rather than a count.
  const claimsBurrowReEncoded =
    /Not Only PDF\s+re-encoded|re-encoded (?:it|your|the file|the document)/i;

  it("never tells a person their document was re-encoded, on any of the three outcomes", () => {
    const outcomes = [
      resultFor(reply(4 * MB, 2 * MB, true)), // smaller
      resultFor(reply(12 * MB, Math.round(12 * MB * 0.9985), true)), // negligible
      resultFor(reply(Math.round(1.2 * MB), Math.round(1.21 * MB), false)), // unchanged
    ];
    expect(outcomes).toHaveLength(3);
    for (const r of outcomes) {
      expect(`${r.title} ${r.detail}`, r.kind).not.toMatch(claimsBurrowReEncoded);
    }
  });

  it("and the rule is a rule, not a coincidence of the current wording", () => {
    // THE PROBE. Without it the pattern above could match nothing at all and the assertion
    // would pass over every future rewording, which is this repository's "4 of 15" failure in
    // a sentence.
    expect("Not Only PDF re-encoded it and got 1.2 MB").toMatch(claimsBurrowReEncoded);
    expect("re-encoded your file at lower quality").toMatch(claimsBurrowReEncoded);
    // And the sentence the page is FOR must survive it.
    expect("Nothing was re-encoded: every page's contents come through unaltered.").not.toMatch(
      claimsBurrowReEncoded,
    );
  });
});

describe("a reply this module cannot read", () => {
  // UNREACHABLE FROM RUST TODAY -- both counts are `u64` strings and the core always sends
  // them. Tested anyway, because the branch exists precisely so a future shape change degrades
  // to the honest answer, and an untested degradation path is a guess about what it does.

  it("still says something true when the numbers are missing", () => {
    const r = resultFor({ originalBytes: "", producedBytes: "", hasDocument: true });
    expect(r.kind).toBe("smaller");
    expect(r.title).toBe("Your compressed document is ready.");
    // AND NO `NaN%`, which is the failure this branch exists to avoid.
    expect(r.title).not.toContain("NaN");
    expect(r.saved).toBe(0);
  });

  it("does not render an empty size where a number should be", () => {
    // `mb()` returns "" for a non-positive count, so an unchanged reply claiming 0 produced
    // bytes would otherwise read "got  against your 1.2 MB" -- a gap where a measurement
    // should be, which is worse than either number.
    const r = resultFor({
      originalBytes: String(Math.round(1.2 * MB)),
      producedBytes: "0",
      hasDocument: false,
    });
    expect(r.kind).toBe("unchanged");
    expect(r.detail).not.toMatch(/came to\s+against/);
  });
});

describe("the threshold between negligible and worthwhile", () => {
  it("is one percent, and the boundary falls the way it is documented", () => {
    // Just under and just over, so a change to the constant is a change to a test rather than
    // something that quietly reclassifies every scan.
    const under = resultFor(
      reply(1000000, Math.round(1000000 * (1 - NEGLIGIBLE_SAVING / 2)), true),
    );
    expect(under.kind).toBe("negligible");
    const over = resultFor(reply(1000000, Math.round(1000000 * (1 - NEGLIGIBLE_SAVING * 2)), true));
    expect(over.kind).toBe("smaller");
  });

  it("only changes wording — the document is offered either way", () => {
    const under = resultFor(reply(1000000, 999000, true));
    const over = resultFor(reply(1000000, 900000, true));
    expect(under.hasDocument).toBe(true);
    expect(over.hasDocument).toBe(true);
  });
});

describe("sizes a person can read", () => {
  it("uses KB below a tenth of a megabyte, rather than showing 0.0 MB twice", () => {
    // A 40 KB file compressed to 28 KB would otherwise read "0.0 MB -> 0.0 MB. 30% smaller.",
    // which looks like a tool that lost the document.
    const r = resultFor(reply(40 * 1024, 28 * 1024, true));
    expect(r.title).toContain("40 KB");
    expect(r.title).toContain("28 KB");
    expect(r.title).not.toContain("0.0 MB");
  });

  it("still uses MB where MB is the readable unit", () => {
    // The boundary in the other direction, so the KB branch cannot swallow ordinary sizes.
    const r = resultFor(reply(4 * MB, 2 * MB, true));
    expect(r.title).toContain("4.0 MB");
    expect(r.title).toContain("2.0 MB");
  });
});

describe("percentages a person can read", () => {
  it("keeps a decimal below ten and drops it above", () => {
    // "0.1%" is the difference between nothing happening and something happening; "31.4%"
    // claims a precision the number does not have.
    // 0.1%, NOT 0.2%: 0.15 is not exactly representable and `toFixed` rounds it down. The
    // assertion records what the function does rather than what the arithmetic suggests --
    // and for a person reading a 0.15% saving, either rendering says the same thing.
    expect(percent(0.0015)).toBe("0.1%");
    expect(percent(0.314)).toBe("31%");
    expect(percent(0.0856 * 10)).toBe("86%");
  });

  it("never reports a negative or nonsensical saving as one", () => {
    expect(percent(-0.2)).toBe("0%");
    expect(percent(Number.NaN)).toBe("0%");
  });
});

describe("failures", () => {
  it("has a branch for every kind the core can produce here", () => {
    // GATED ON THE COUNT as well as on each one, so a kind dropped from this list would take
    // its assertion with it and the suite would still pass.
    const kinds = [
      "PasswordRequired",
      "Malformed",
      "Unsupported",
      "LimitExceeded",
      "InvalidArgument",
      "OutputRejected",
      "EngineUnavailable",
      "Io",
      "Internal",
    ];
    expect(kinds).toHaveLength(9);
    for (const kind of kinds) {
      const m = messageFor({ kind });
      expect(m.title, kind).not.toBe("");
      expect(m.next, kind).not.toBe("");
      // NO APOLOGIES, from the design brief.
      expect(m.title.toLowerCase(), kind).not.toContain("sorry");
    }
  });

  it("says something of its own, rather than falling through to the fallback", () => {
    // THE STRONGER HALF OF THE PATTERN, which this file was written without. Code review
    // measured the gap on this very file: renaming five branches -- PasswordRequired,
    // Malformed, Unsupported, InvalidArgument and OutputRejected -- so that all five fell
    // through to "Something inside burrow failed" left every test above passing. The loop
    // before this one requires only a non-empty title and no apology, and the fallback
    // satisfies both, so it is a rule that matches everything, which is not a rule.
    //
    // That is the same defect a review caught in `merge-messages.test.ts` and then again in
    // `rotate-messages.test.ts`, each time by copying the weaker half of the previous file.
    // Third time; this is the port of the stronger half.
    //
    // Two kinds SHARE the fallback deliberately, named rather than excluded by a predicate, so
    // adding a kind means deciding which side it is on: `Io` and `Internal` are not about the
    // person's file and there is nothing different to say about them.
    const fallback = messageFor({ kind: "Internal" }).title;
    const sharesTheFallback = ["Io", "Internal"];
    const kinds = [
      "PasswordRequired",
      "Malformed",
      "Unsupported",
      "LimitExceeded",
      "InvalidArgument",
      "OutputRejected",
      "EngineUnavailable",
      "Io",
      "Internal",
    ];

    for (const kind of kinds.filter((k) => !sharesTheFallback.includes(k))) {
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

    // And distinct from EACH OTHER, not merely from the fallback: two branches returning the
    // same sentence would pass every assertion above.
    const distinct = new Set(kinds.map((k) => messageFor({ kind: k }).title));
    expect(
      distinct.size,
      `${kinds.length} kinds, ${sharesTheFallback.length} of them sharing one sentence`,
    ).toBe(kinds.length - sharesTheFallback.length + 1);
  });

  it("names both numbers when a ceiling gave them", () => {
    const m = messageFor({
      kind: "LimitExceeded",
      limit: "max_input_bytes",
      requested: String(600 * MB),
      allowed: String(512 * MB),
    });
    expect(m.title).toContain("600 MB");
    expect(m.title).toContain("512 MB");
  });

  it("does not claim a file was unread when it was", () => {
    // `max_pages` is enforced after the document is in the engine's memory, unlike
    // `max_input_bytes`, which is checked from `Blob.size`. The same overclaim code review
    // caught on rotate.
    const pages = messageFor({
      kind: "LimitExceeded",
      limit: "max_pages",
      requested: "20000",
      allowed: "10000",
    });
    expect(pages.next).not.toContain("Nothing was read");
    expect(pages.next).toContain("opened but not changed");
  });

  it("only EngineUnavailable needs a deliberate gesture", () => {
    // `retryable: false` renders the Start again button, which clears the circuit breaker
    // (ADR 0015 §3). Offering it when the breaker has not latched would be the page lying
    // about its own state.
    expect(messageFor({ kind: "EngineUnavailable" }).retryable).toBe(false);
    for (const kind of ["Malformed", "OutputRejected", "Internal", "LimitExceeded"]) {
      expect(messageFor({ kind }).retryable, kind).toBe(true);
    }
  });

  it("an unknown kind still arrives as something a person can act on", () => {
    // `burrow_types::Error` is `#[non_exhaustive]`.
    const m = messageFor({ kind: "SomethingAddedUpstream" });
    expect(m.title).not.toBe("");
    expect(m.next).toContain("nothing was sent anywhere");
  });
});
