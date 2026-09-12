// The web half of the differential conformance harness. ROADMAP M1 item 12.
//
// Every corpus file, through both engines, in every browser — and then diffed against what the
// native implementation produced for the same corpus.
//
// THROUGH THE REAL HARNESS, WITH NOTHING SWITCHED OFF
//
// `window.burrowHarness` → `worker-host.js` → a `blob:` worker under the generated CSP, with
// the fail-closed policy guard, the watchdog, recovery and heap recycling all live. There is
// deliberately no shortcut that talks to the wasm module directly: the thing under test is the
// path that ships, and a harness that bypassed the host would stop testing it the first time
// the host changed.
//
// WHY THIS EXISTS AT ALL
//
// `core/burrow-engines/src/web/mod.rs` states it plainly: the web path's orchestration is
// shared Rust, but the two `DocumentEngine` implementations are separate, "and at M2 a
// divergence between them is a redaction bug rather than a test failure". Until this file
// existed, the adversarial corpus — the xref bomb, the three pre-scan bypasses security review
// found, the file that used to abort the process, the two files qpdf reads and PDFium refuses —
// had only ever been run natively.

import { readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

import {
  compare,
  parseAllowlist,
  SUPPORTED_SCHEMA,
  type CaseLimits,
  type ErrorKind,
  type Expectations,
  type Operation,
  type OutcomeRecord,
  type Outcome,
  type RecordedOutcome,
} from "../src/conformance/compare.js";
import { openHarness, type Reply } from "./harness";

const here = dirname(fileURLToPath(import.meta.url));
const conformance = resolve(here, "../../../tests/conformance");

const rawExpectations = readFileSync(join(conformance, "expectations.json"), "utf8");
const expectations = JSON.parse(rawExpectations) as Expectations;
const expectationsDigest = createHash("sha256").update(rawExpectations).digest("hex");

// A newer schema means the shape changed. Fail rather than guess — the same rule the Rust
// reader follows, and for the same reason: a silently misread expectation is worse than none.
if (expectations.schema !== SUPPORTED_SCHEMA) {
  throw new Error(`unsupported expectations schema ${expectations.schema}`);
}

const OPERATIONS: Operation[] = ["page_count", "structure_check"];

/**
 * Turn a reply into the shape the schema records.
 *
 * Mirrors `outcome_of` in `testsupport/expectations.rs`, including dropping `requested` at the
 * `measured` stage — that number is the process resident set on native and `HEAPU8.byteLength`
 * here (ADR 0007), so recording it would guarantee a false divergence on every run.
 */
function exact(value: string): number {
  // These are `u64` and reach the page as strings precisely because they can exceed 2^53 --
  // `estimated_open_bytes` saturates, so a value above it is reachable. `Number()` would round
  // one silently, and a rounded number compared against an exact one is a divergence with no
  // cause. Nothing in today's corpus comes close; this fails loudly rather than waiting.
  const asNumber = Number(value);
  if (!Number.isSafeInteger(asNumber)) {
    throw new Error(
      `${value} cannot be compared exactly as a JavaScript number. The schema would need to ` +
        `record these as strings before a fixture can rely on a value this large.`,
    );
  }
  return asNumber;
}

function outcomeOf(reply: Reply): Outcome {
  if (reply.ok) {
    return { ok: { page_count: reply.pages } };
  }
  if (reply.kind === "LimitExceeded") {
    return {
      err: {
        kind: "LimitExceeded",
        limit: reply.limit,
        stage: reply.stage,
        ...(reply.stage === "measured" ? {} : { requested: exact(reply.requested) }),
        allowed: exact(reply.allowed),
      },
    };
  }
  // Every other variant records only its kind. The message is deliberately not part of the
  // contract: a message is ours to reword, a variant is what both implementations must agree
  // on, and a message is the one field that could carry something input-derived.
  return { err: { kind: reply.kind as ErrorKind } };
}

test("every corpus file produces the same typed outcome as the native path", async ({
  page,
}, testInfo) => {
  await openHarness(page);

  const results: RecordedOutcome[] = [];

  for (const testCase of expectations.cases) {
    const bytes = readFileSync(join(conformance, testCase.file));
    // The digest is CHECKED, not just printed. Failure messages quote `case.sha256` as though
    // it described what ran; the native side verifies that and the web side did not, so the
    // line was a claim this half had not established. Same-commit checkouts make it moot in
    // CI, which is exactly when an unverified claim survives longest.
    expect(
      createHash("sha256").update(bytes).digest("hex"),
      `${testCase.file} does not match the digest recorded for it`,
    ).toBe(testCase.sha256);
    const base64 = bytes.toString("base64");
    for (const operation of OPERATIONS) {
      // A FRESH WORKER FOR EVERY CASE, and it is a correctness requirement rather than
      // hygiene.
      //
      // WASM linear memory never shrinks, so `check_measured_memory` on the web measures how
      // far the heap *grew* — and a heap that already holds 200 MiB from a previous case can
      // satisfy the next one without growing at all. Running the corpus in one worker made
      // `objstm-bomb-tight-ceiling` return `Ok` because the case before it had already paid
      // for the heap, and made its sibling operation return `LimitExceeded` because a recycle
      // in between had reset one of the two engines. The outcome depended on the order of the
      // corpus, which is not an outcome at all.
      //
      // Native gets this for free: each case opens against a process whose resident set is
      // whatever it is, and only the delta counts. Discarding here buys the same independence
      // for about 80 ms a case (ADR 0015 §6).
      await page.evaluate(() => window.burrowHarness.discardWorker());

      const reply = await page.evaluate(
        ([op, bytes, password, attemptRecovery, limits]) =>
          window.burrowHarness.runBase64(op as "page_count" | "structure_check", bytes as string, {
            password: password as string | null,
            attemptRecovery: attemptRecovery as boolean,
            limits: limits as Record<string, number>,
          }),
        [
          operation,
          base64,
          testCase.password,
          testCase.attempt_recovery ?? false,
          // `max_duration_ms` is raised for every case, and that is what makes this
          // reproducible. Native injects a `ManualClock` that never advances; the web cannot,
          // so a generous budget is the only way to guarantee no outcome here is
          // timing-derived. Nothing in this corpus is meant to hit a duration limit, and a
          // slow CI runner must not invent one.
          { maxDurationMs: 600_000, ...camelCaseLimits(testCase.limits) },
        ] as const,
      );
      // NO CORPUS FILE MAY COST A WORKER. `engines.spec.ts`'s old loop asserted this and
      // `outcomeOf` does not record it, so without this line it would have been dropped
      // silently — and a fatal reply would not otherwise fail anything here, because each case
      // discards its worker anyway.
      expect(
        reply.fatal,
        `${testCase.name} (${operation}): a corpus file must not poison the instance`,
      ).toBe(false);

      results.push({ case: testCase.name, operation, outcome: outcomeOf(reply) });
    }
  }

  const record: OutcomeRecord = {
    platform: "web",
    runner: testInfo.project.name,
    expectations_sha256: expectationsDigest,
    results,
  };
  writeFileSync(
    join(testInfo.project.outputDir, `conformance.${testInfo.project.name}.json`),
    `${JSON.stringify(record, null, 2)}\n`,
  );

  // ---- the diff --------------------------------------------------------------------
  //
  // The native record must exist. Absent, it is not "nothing to compare" — it is a corpus
  // that was never run on one of the two paths, which is the exact thing this harness is for.
  const nativeRecordPath = join(conformance, "native-outcomes.json");
  let nativeRaw: string;
  try {
    nativeRaw = readFileSync(nativeRecordPath, "utf8");
  } catch {
    throw new Error(
      `${nativeRecordPath} is missing. It is written by \`cargo test -p burrow-engines ` +
        `--all-features --test conformance\`, which must run before this spec. A missing ` +
        `native record is a corpus that ran on one path only, not a comparison with nothing ` +
        `to say.`,
    );
  }
  const native = JSON.parse(nativeRaw) as OutcomeRecord;

  const allowlistText = readFileSync(join(conformance, "divergences.toml"), "utf8");
  const allowlist = parseAllowlist(allowlistText);
  expect(allowlist.errors, "the divergence allowlist does not parse").toEqual([]);

  const verdict = compare(
    expectations,
    native,
    record,
    allowlist.entries,
    // Computed here, over the exact bytes this spec parsed. Passing it in rather than letting
    // the comparator trust the records means a record written against a different corpus fails
    // instead of comparing cleanly against files it never ran.
    expectationsDigest,
  );

  if (verdict.gaps.length > 0) {
    // Printed on every run. A known gap is green CI and an open defect at the same time, and
    // the only thing keeping it visible is that it says so out loud.
    console.log(`\nknown gaps in this corpus (${verdict.gaps.length}):`);
    for (const gap of verdict.gaps) console.log(`  ${gap}`);
  }
  if (verdict.allowed.length > 0) {
    console.log(`\nallowlisted divergences (${verdict.allowed.length}):`);
    for (const entry of verdict.allowed) console.log(`  ${entry}`);
  }

  expect(verdict.failures.join("\n\n"), "native and web disagree").toBe("");
  expect(
    verdict.stale.join("\n\n"),
    "a recorded difference no longer happens, so the record is stale",
  ).toBe("");
  expect(verdict.comparisons, "the harness compared nothing").toBe(
    expectations.cases.length * OPERATIONS.length,
  );
});

/** `expectations.json` uses `snake_case`; `HarnessLimits` is `camelCase`. */
function camelCaseLimits(limits: CaseLimits | undefined): Record<string, number> {
  if (!limits) return {};
  const out: Record<string, number> = {};
  for (const [key, value] of Object.entries(limits) as [string, number][]) {
    out[key.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase())] = value;
  }
  return out;
}
