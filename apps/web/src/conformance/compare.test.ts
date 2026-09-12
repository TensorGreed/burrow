// The comparator, driven with planted failures.
//
// A differential harness is the most tempting possible shape for a check that reports success
// while checking nothing: "no divergences found" and "no comparisons made" render identically.
// Every defence in `compare.ts` exists because of that, and every one of them is exercised here
// against data built to trip it — rather than only end-to-end, where a defence that had stopped
// working would look exactly like a corpus that had nothing wrong with it.
//
// The two planted divergences are the headline. They are separate cases, not one parameterised
// case, because they take different branches: a native-side divergence is caught by the
// native-vs-corpus assertion first, and a web-side one by the web-vs-corpus assertion — and a
// comparator that had lost one of those would still catch the other.

import { describe, expect, test } from "vitest";

import {
  compare,
  parseAllowlist,
  render,
  type Case,
  type Divergence,
  type Expectations,
  type Outcome,
  type OutcomeRecord,
} from "./compare.js";

const OK: Outcome = { ok: { page_count: 1 } };
const MALFORMED: Outcome = { err: { kind: "Malformed" } };
const PRESCAN: Outcome = {
  err: {
    kind: "LimitExceeded",
    limit: "max_memory_bytes",
    stage: "prescan",
    requested: 1_280_000_000,
    allowed: 1_073_741_824,
  },
};
/** The same kind and limit, reached by a different route. */
const MEASURED: Outcome = {
  err: {
    kind: "LimitExceeded",
    limit: "max_memory_bytes",
    stage: "measured",
    allowed: 1_073_741_824,
  },
};

function corpus(cases: Partial<Case>[]): Expectations {
  return {
    schema: 2,
    generated_by: "test",
    current_milestone: "M1",
    cases: cases.map((c, i) => ({
      name: c.name ?? `case-${i}`,
      file: c.file ?? `fixtures/case-${i}.pdf`,
      sha256: c.sha256 ?? "0".repeat(64),
      password: c.password ?? null,
      expect: c.expect ?? { page_count: OK, structure_check: OK },
      ...c,
    })) as Case[],
  };
}

/** A record that answers every case with what the corpus expects for that platform. */
function agreeing(
  expectations: Expectations,
  platform: "native" | "web",
  runner: string = platform,
): OutcomeRecord {
  return {
    platform,
    runner,
    expectations_sha256: "digest",
    results: expectations.cases.flatMap((c) => [
      {
        case: c.name,
        operation: "page_count" as const,
        outcome:
          (c.platform_expectations ?? []).find(
            (p) => p.platform === platform && p.operation === "page_count",
          )?.expect ?? c.expect.page_count,
      },
      {
        case: c.name,
        operation: "structure_check" as const,
        outcome:
          (c.platform_expectations ?? []).find(
            (p) => p.platform === platform && p.operation === "structure_check",
          )?.expect ?? c.expect.structure_check,
      },
    ]),
  };
}

/** The digest the records are written against, and what `compare` is told the corpus is. */
const DIGEST = "digest";

function run(
  expectations: Expectations,
  mutate: (n: OutcomeRecord, w: OutcomeRecord) => void = () => {},
  allowlist: Divergence[] = [],
) {
  const native = agreeing(expectations, "native");
  const web = agreeing(expectations, "web", "chromium");
  mutate(native, web);
  return compare(expectations, native, web, allowlist, DIGEST);
}

describe("the happy path", () => {
  test("an agreeing corpus produces no failures and a full comparison count", () => {
    const expectations = corpus([
      { name: "a" },
      { name: "b", expect: { page_count: OK, structure_check: MALFORMED } },
    ]);
    const verdict = run(expectations);
    expect(verdict.failures).toEqual([]);
    expect(verdict.stale).toEqual([]);
    expect(verdict.comparisons).toBe(4);
  });
});

// =====================================================================================
// The planted divergences
// =====================================================================================

describe("a planted divergence is caught", () => {
  test("on the NATIVE side", () => {
    const expectations = corpus([{ name: "a" }]);
    const verdict = run(expectations, (native) => {
      native.results[0].outcome = MALFORMED;
    });
    expect(verdict.failures.join("\n")).toContain("native did not match the corpus");
    expect(verdict.failures.join("\n")).toContain("the two implementations disagree");
    // Still counted. A divergence is a comparison that was made, not one that was skipped.
    expect(verdict.comparisons).toBe(2);
  });

  test("on the WEB side", () => {
    const expectations = corpus([{ name: "a" }]);
    const verdict = run(expectations, (_native, web) => {
      web.results[0].outcome = MALFORMED;
    });
    expect(verdict.failures.join("\n")).toContain("web (chromium) did not match the corpus");
    expect(verdict.failures.join("\n")).toContain("the two implementations disagree");
    expect(verdict.comparisons).toBe(2);
  });

  test("even when both sides agree with each other but not with the corpus", () => {
    // The case a direct diff alone cannot catch, and the reason both mechanisms are here.
    const expectations = corpus([{ name: "a" }]);
    const verdict = run(expectations, (native, web) => {
      native.results[0].outcome = MALFORMED;
      web.results[0].outcome = MALFORMED;
    });
    expect(verdict.failures.length).toBe(2);
    expect(verdict.failures.join("\n")).toContain("native did not match the corpus");
    expect(verdict.failures.join("\n")).toContain("web (chromium) did not match the corpus");
  });

  test("the SAME error kind reached by a different route", () => {
    // The whole reason `Stage` was added to `LimitExceeded`. Without it these two outcomes
    // are identical, and a rejection moving from the pre-scan to the measured check — from
    // "refused before anything parsed it" to "refused after a gigabyte was allocated" —
    // would be invisible.
    const expectations = corpus([
      { name: "bomb", expect: { page_count: PRESCAN, structure_check: PRESCAN } },
    ]);
    const verdict = run(expectations, (_native, web) => {
      web.results[0].outcome = MEASURED;
    });
    expect(verdict.failures.join("\n")).toContain("the two implementations disagree");
    expect(verdict.failures.join("\n")).toContain("stage=prescan");
    expect(verdict.failures.join("\n")).toContain("stage=measured");
  });
});

// =====================================================================================
// Defences against a vacuous pass
// =====================================================================================

describe("the harness cannot pass while checking nothing", () => {
  test("an empty corpus fails rather than comparing nothing", () => {
    const verdict = run(corpus([]));
    expect(verdict.comparisons).toBe(0);
    expect(verdict.failures.join("\n")).toContain("the corpus is empty");
    expect(verdict.failures.join("\n")).toContain("no comparisons were made at all");
  });

  test("a missing result on one side is a failure, never agreement", () => {
    const expectations = corpus([{ name: "a" }, { name: "b" }]);
    const verdict = run(expectations, (_native, web) => {
      web.results = web.results.filter((r) => r.case !== "b");
    });
    expect(verdict.failures.join("\n")).toContain("web produced no result");
    expect(verdict.failures.join("\n")).toContain("not the same as the two paths agreeing");
    expect(verdict.comparisons).toBe(2);
    expect(verdict.failures.join("\n")).toContain("only 2 of 4 comparisons");
  });

  test("BOTH sides missing the same case is still a failure", () => {
    // The shape that would otherwise look perfect: two empty records compare equal.
    const expectations = corpus([{ name: "a" }]);
    const verdict = run(expectations, (native, web) => {
      native.results = [];
      web.results = [];
    });
    expect(verdict.comparisons).toBe(0);
    expect(verdict.failures.join("\n")).toContain("no comparisons were made at all");
  });

  test("THE SAME RECORD PASSED TWICE is refused, not agreed with", () => {
    // Found by both reviewers independently, and each demonstrated it: this used to return
    // `{comparisons: 4, failures: []}` — a full comparison count and perfect agreement, from a
    // harness that had compared one implementation with itself. `platform` was in the type,
    // present in both records, and read only to render a message.
    const expectations = corpus([{ name: "a" }, { name: "b" }]);
    const web = agreeing(expectations, "web", "chromium");
    const verdict = compare(expectations, web, web, [], DIGEST);
    expect(verdict.comparisons).toBe(0);
    expect(verdict.failures.join("\n")).toContain("needs one of each");
  });

  test("a record from the wrong platform is refused either way round", () => {
    const expectations = corpus([{ name: "a" }]);
    const native = agreeing(expectations, "native");
    const web = agreeing(expectations, "web", "chromium");
    // Arguments swapped: both records are real, and the comparison is still meaningless.
    expect(compare(expectations, web, native, [], DIGEST).failures.join("\n")).toContain(
      "needs one of each",
    );
  });

  test("a duplicate row cannot last-wins its way past a divergence", () => {
    // `new Map(results.map(...))` is last-wins, so a wrong row followed by a right one for the
    // same key would compare clean. Counted rather than deduplicated: a record with the wrong
    // number of rows is not a record to reason about.
    const expectations = corpus([{ name: "a" }]);
    const verdict = run(expectations, (_native, web) => {
      web.results.unshift({ case: "a", operation: "page_count", outcome: MALFORMED });
    });
    expect(verdict.failures.join("\n")).toContain("distinct (case, operation) pairs");
  });

  test("BOTH records carrying the same WRONG digest is refused", () => {
    // The hole in the old freshness check: it compared each record to `web`, so the `web`
    // iteration was a no-op and it asserted only that the two agreed with each other. Two
    // records written against a corpus neither of them ran passed clean.
    const expectations = corpus([{ name: "a" }]);
    const verdict = run(expectations, (native, web) => {
      native.expectations_sha256 = "a-different-corpus";
      web.expectations_sha256 = "a-different-corpus";
    });
    expect(verdict.failures.filter((f) => f.includes("that record is stale"))).toHaveLength(2);
  });

  test("a missing result on the NATIVE side is caught too", () => {
    // The mirror of the web-side case below. A comparator narrowed to `if (w === undefined)`
    // passed every other test in this file, because nothing ever removed a native row.
    const expectations = corpus([{ name: "a" }, { name: "b" }]);
    const verdict = run(expectations, (native) => {
      native.results = native.results.filter((r) => r.case !== "b");
    });
    expect(verdict.failures.join("\n")).toContain("native produced no result");
    expect(verdict.comparisons).toBe(2);
  });

  test("a stale record is refused", () => {
    const expectations = corpus([{ name: "a" }]);
    const verdict = run(expectations, (native) => {
      native.expectations_sha256 = "a-different-corpus";
    });
    expect(verdict.failures.join("\n")).toContain("that record is stale");
  });

  test("an unknown schema stops the comparison instead of guessing", () => {
    const expectations = corpus([{ name: "a" }]);
    expectations.schema = 99;
    const verdict = run(expectations);
    expect(verdict.comparisons).toBe(0);
    expect(verdict.failures.join("\n")).toContain("schema 99");
  });
});

// =====================================================================================
// The two lists, and the difference between them
// =====================================================================================

describe("platform expectations", () => {
  const byDesign: Partial<Case> = {
    name: "hangs",
    expect: { page_count: MALFORMED, structure_check: OK },
    platform_expectations: [
      {
        platform: "web",
        operation: "page_count",
        expect: OK,
        reason:
          "the main-thread watchdog terminates the worker; the native path cannot interrupt " +
          "a single engine call (ADR 0007, ADR 0015 §2)",
      },
    ],
  };

  test("a by-design difference is expected, not merely tolerated", () => {
    const verdict = run(corpus([byDesign]));
    expect(verdict.failures).toEqual([]);
    expect(verdict.comparisons).toBe(2);
  });

  test("a by-design difference that stops happening is a stale record", () => {
    // A recorded difference nobody removed is something the next reader will believe.
    const expectations = corpus([byDesign]);
    const verdict = run(expectations, (_native, web) => {
      web.results[0].outcome = MALFORMED; // now matching native
    });
    expect(verdict.stale.join("\n")).toContain("no longer does");
  });

  test("a platform expectation does NOT excuse a different outcome from the one recorded", () => {
    const expectations = corpus([byDesign]);
    const verdict = run(expectations, (_native, web) => {
      web.results[0].outcome = { ok: { page_count: 99 } };
    });
    expect(verdict.failures.join("\n")).toContain("web (chromium) did not match the corpus");
  });
});

describe("the divergence allowlist", () => {
  const waiver: Divergence = {
    case: "a",
    operation: "page_count",
    reason: "PDFium and qpdf disagree about this file for a reason nobody has chased yet",
    issue: "https://github.com/TensorGreed/burrow/issues/1",
  };

  test("an allowlisted divergence is reported, not hidden", () => {
    const expectations = corpus([{ name: "a" }]);
    const verdict = run(
      expectations,
      (_native, web) => {
        web.results[0].outcome = MALFORMED;
      },
      [waiver],
    );
    // Still fails the corpus assertion -- the allowlist waives the DIFFERENCE between the two
    // implementations, never the contract. Conflating those would let one entry silence a real
    // behaviour change.
    expect(verdict.failures.join("\n")).toContain("web (chromium) did not match the corpus");
    expect(verdict.allowed.join("\n")).toContain("issues/1");
  });

  test("an allowlist entry naming a case that does not exist fails", () => {
    const verdict = run(corpus([{ name: "a" }]), () => {}, [{ ...waiver, case: "typo" }]);
    expect(verdict.failures.join("\n")).toContain("which is not in the corpus");
  });

  test("an allowlist entry whose divergence never happens is stale", () => {
    const verdict = run(corpus([{ name: "a" }]), () => {}, [waiver]);
    expect(verdict.stale.join("\n")).toContain("no longer occurs");
  });

  test("a waiver made redundant by a platform expectation is stale", () => {
    // The one branch in `compare.ts` that had no test, and working out how to reach it is the
    // point: a key whose divergence is already explained BY DESIGN never reaches the allowlist
    // at all, so a waiver for it matches nothing and would sit in the file forever looking
    // load-bearing. That is how a debt gets recorded twice and paid off zero times.
    const expectations = corpus([
      {
        name: "a",
        expect: { page_count: MALFORMED, structure_check: OK },
        platform_expectations: [
          {
            platform: "web",
            operation: "page_count",
            expect: OK,
            reason:
              "the main-thread watchdog terminates the worker; the native path cannot " +
              "interrupt a single engine call (ADR 0007, ADR 0015 §2)",
          },
        ],
      },
    ]);
    const verdict = run(expectations, () => {}, [{ ...waiver, operation: "page_count" }]);
    expect(verdict.failures, "the by-design difference itself is fine").toEqual([]);
    expect(verdict.stale.join("\n")).toContain("never occurred");
  });
});

describe("parsing the allowlist", () => {
  test("the empty list parses to nothing, with no errors", () => {
    const { entries, errors } = parseAllowlist("# a comment\n\n");
    expect(entries).toEqual([]);
    expect(errors).toEqual([]);
  });

  test("a complete entry parses", () => {
    const { entries, errors } = parseAllowlist(
      `[[divergence]]\ncase = "a"\noperation = "page_count"\n` +
        `reason = "a reason long enough to actually explain the difference to a reader"\n` +
        `issue = "https://example.com/1"\n`,
    );
    expect(errors).toEqual([]);
    expect(entries).toHaveLength(1);
    expect(entries[0].case).toBe("a");
  });

  test.each([
    [
      "a missing issue",
      `[[divergence]]\ncase = "a"\noperation = "page_count"\nreason = "${"x".repeat(50)}"\n`,
    ],
    [
      "a missing reason",
      `[[divergence]]\ncase = "a"\noperation = "page_count"\nissue = "https://e/1"\n`,
    ],
    [
      "a label instead of a reason",
      `[[divergence]]\ncase = "a"\noperation = "page_count"\nreason = "engine bug"\nissue = "https://e/1"\n`,
    ],
    [
      "an unknown operation",
      `[[divergence]]\ncase = "a"\noperation = "render"\nreason = "${"x".repeat(50)}"\nissue = "https://e/1"\n`,
    ],
    [
      "an issue that is not a link",
      `[[divergence]]\ncase = "a"\noperation = "page_count"\nreason = "${"x".repeat(50)}"\nissue = "see slack"\n`,
    ],
  ])("%s is refused", (_what, text) => {
    // An entry that cannot explain itself is not a waiver; it is a divergence someone did not
    // want to write down.
    expect(parseAllowlist(text).errors.length).toBeGreaterThan(0);
  });
});

// =====================================================================================
// CI output carries no file content
// =====================================================================================

describe("failure output", () => {
  test("renders typed detail and nothing else", () => {
    expect(render(OK)).toBe("ok(1 pages)");
    expect(render(MALFORMED)).toBe("Malformed");
    expect(render(PRESCAN)).toBe(
      "LimitExceeded(limit=max_memory_bytes stage=prescan requested=1280000000 allowed=1073741824)",
    );
    expect(render(undefined)).toBe("<no result>");
  });

  test("a divergence message contains no fixture bytes and no engine message", () => {
    // `secret_leak.rs`'s discipline, applied to CI logs. A conformance failure is exactly when
    // somebody pastes output into an issue, and a fixture is a user's file in every way that
    // matters to this rule.
    const expectations = corpus([
      { name: "canary", file: "fixtures/canary.pdf", sha256: "abc123" },
    ]);
    const verdict = run(expectations, (_native, web) => {
      web.results[0].outcome = MALFORMED;
    });
    const text = verdict.failures.join("\n");
    expect(text).toContain("fixtures/canary.pdf");
    expect(text).toContain("abc123");
    // The renderers above are the only thing that ever reaches a message, and they emit a
    // fixed vocabulary. Nothing input-derived can appear, because nothing input-derived is in
    // an `Outcome` at all.
    expect(text).not.toMatch(/%PDF|BURROW-CANARY|offset \d|object \d+ \d+/);
  });
});
