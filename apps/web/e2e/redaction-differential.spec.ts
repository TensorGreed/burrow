// #137's browser differential: every redaction `tests/redaction/outcomes.tsv` pins, run through
// redaction's own worker in each browser and held to the native outcome.
//
// The golden file is written by `core/burrow-ops/tests/redaction_outcomes.rs` from the NATIVE
// engines; CI checks it there on every run. This runs the same cases through the WEB engines --
// the wasm qpdf, the `redact` binding, the worker glue -- so a divergence between the two
// implementations of one policy is a failure here, by document and by case.
//
// THE CORPUS IS THE GOLDEN FILE'S, all of it. `tests/redaction/generated/` is gitignored: CI's
// `test` job builds it with the vendored `qpdf` and `cjpeg`, which the `web` job does not have, and
// hands it over as an artifact -- the pattern `native-conformance-record` already uses. Locally,
// `tools/check-redaction-corpus.sh` builds it. A document that is missing, or is not the one the
// golden file recorded, FAILS BY NAME; nothing is skipped silently, because a differential that
// quietly compared fewer documents would read exactly like one that compared them all.
//
// THE LEDGER is what makes that sentence checked rather than claimed. Each document test records,
// for every case it ran, what the web was OBSERVED to reply -- kind, rule name, ceiling fields,
// digests; no prose, no bytes -- and the ledger judges each observation itself, against the
// outcome the golden file records under that case's name, read a second time independently of the
// parse the tests iterate. The LAST test in the file, per browser, does that; so does
// `tools/check-redaction-differential-ledger.sh`, from OUTSIDE the spec, over the same records,
// because a test inside it can be parked. It reports "N compared, D diverged, M skipped" and fails
// by name on any case not compared, or compared and diverged, unless it is in `DECLARED_SKIPS`
// with a reason. So a document test that is parked (`test.fail()`, a runtime `test.skip`), or
// that judges against the wrong case, does not decide the verdict; and parking the ledger test as
// well leaves the checker. What the ledger trusts is that an observation came from the worker.
//
// It relies on the file's tests running in order in one worker per project -- `fullyParallel:
// false` in `playwright.config.ts` -- and on no test running twice. EVERY DEPARTURE FAILS CLOSED:
// retries or `--repeat-each` read as "compared 2 times", parallelism within the file as "not
// compared". The one that does not is outside CI: Playwright's UI mode keeps the output directory
// between runs, so re-running the ledger test ALONE there reads the previous run's record. Run the
// file, or use the command line.
//
// `tools/test-redaction-differential-ledger.sh` plants silent drops, parked divergences and a
// verdict bound to the wrong case in copies of this spec, and requires the ledger -- and the
// checker -- to refuse each by name.
//
// WHAT IS COMPARED is `redaction-differential.ts`' header: both digests of a redaction, and a
// refusal's typed kind and rule name -- never its prose.

import { createHash } from "node:crypto";
import { appendFileSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { basename, dirname, join } from "node:path";

import { expect, test, type TestInfo } from "@playwright/test";

import type { Reply } from "../src/host/harness-api";
import { openHarness } from "./harness";
import {
  type Case,
  type Class,
  caseName,
  classOf,
  DECLARED_SKIPS,
  divergence,
  documentBytes,
  golden,
  goldenCases,
  judge,
  type Observed,
  observe,
  planned,
  reconcile,
  type Recorded,
  ruleOf,
  STAGES,
} from "./redaction-differential";

const DOCUMENTS = golden();

/** Each document with the cases the differential runs; a document with none is not run. */
const PLAN = DOCUMENTS.map((d) => ({ ...d, cases: planned(d) })).filter((d) => d.cases.length > 0);

/**
 * This run's ledger for this spec file in this browser. Under the project's output directory,
 * which Playwright empties at the start of a run, and reset again by the first test here, so a
 * ledger from an earlier run cannot vouch for this one.
 */
function ledgerPath(info: TestInfo): string {
  return join(
    info.project.outputDir,
    "redaction-differential-ledger",
    `${basename(info.file)}.${info.project.name}.jsonl`,
  );
}

function record(info: TestInfo, observations: Recorded[]): void {
  const path = ledgerPath(info);
  mkdirSync(dirname(path), { recursive: true });
  appendFileSync(path, observations.map((o) => JSON.stringify(o) + "\n").join(""));
}

/** Every case of one document, through the harness's redaction worker, in the golden file's order. */
async function runCases(
  page: import("@playwright/test").Page,
  bytes: Buffer,
  cases: Case[],
): Promise<Reply[]> {
  return page.evaluate(
    async ({ base64, cases }) => {
      const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
      const replies = [];
      for (const c of cases) {
        // 1-BASED ON THE WEB, as every operation's page numbers are; the golden file is 0-based.
        replies.push(
          await window.burrowHarness.redactDocument(
            "differential.pdf",
            bytes,
            c.page + 1,
            c.covered.map((n) => n + 1),
            c.region,
          ),
        );
      }
      return replies;
    },
    { base64: bytes.toString("base64"), cases },
  );
}

test("the corpus on disk is the one the golden file records, all of it", () => {
  rmSync(ledgerPath(test.info()), { force: true });
  // THE COUNTS, AND WHAT THEY SHOULD BE, both from the golden file: a run over fewer documents
  // or cases reads as fewer, not as success.
  const cases = DOCUMENTS.flatMap((d) => d.cases);
  // EVERY DOCUMENT THE PLAN RUNS must be the recorded one; a whole document declared skipped is
  // exempt, and the ledger names it.
  const problems = PLAN.map(documentBytes).flatMap((r) => ("problem" in r ? [r.problem] : []));
  const classes = new Map<Class, number>();
  for (const c of cases)
    classes.set(classOf(c.outcome), (classes.get(classOf(c.outcome)) ?? 0) + 1);
  test.info().annotations.push({
    type: "corpus",
    description:
      `${DOCUMENTS.length} documents, ${cases.length} cases: ` +
      [...classes].map(([k, n]) => `${n} ${k}`).join(", "),
  });
  expect(problems, "run tools/check-redaction-corpus.sh, or download the CI artifact").toEqual([]);
  expect(DOCUMENTS.length, "the golden file records no documents").toBeGreaterThan(0);
  // EVERY DIRECTORY THE NATIVE TEST READS, so a whole one missing from the golden file is loud.
  for (const directory of [
    "tests/redaction/fixtures/",
    "tests/redaction/generated/",
    "tests/conformance/fixtures/",
    "tests/damaged/",
  ]) {
    expect(
      DOCUMENTS.some((d) => d.name.startsWith(directory)),
      `the golden file records nothing from ${directory}`,
    ).toBe(true);
  }
});

test("SHOWN TO FAIL, by hand: a document that is missing, or is not the recorded one, is named", () => {
  const recorded = DOCUMENTS[0];
  const missing = documentBytes({ ...recorded, name: "tests/redaction/generated/nowhere.pdf" });
  expect("problem" in missing && missing.problem).toMatch(/nowhere\.pdf is not on disk/);
  const different = documentBytes({ ...recorded, digest: "0".repeat(64) });
  expect("problem" in different && different.problem).toMatch(/is not the document the golden/);
  expect("bytes" in documentBytes(recorded), "the near-miss: the recorded document").toBe(true);
});

for (const document of PLAN) {
  test(`every recorded redaction of ${document.name} is the native one`, async ({ page }) => {
    const cases = document.cases;
    const found = documentBytes(document);
    if ("problem" in found) throw new Error(found.problem);
    await openHarness(page);
    const replies = await runCases(page, found.bytes, cases);
    expect(replies, "a case produced no reply").toHaveLength(cases.length);

    // WHAT WAS OBSERVED, per case, is what is recorded -- before this test's own verdict, and
    // judged again by the ledger against its own read of each outcome. This test's verdict is
    // made from the same record, so the two cannot be handed different replies.
    const compared = cases.map((c, i) => ({
      name: caseName(document.name, c),
      observed: observe(replies[i]),
    }));
    record(test.info(), compared);
    const divergences = compared.flatMap((v, i) => {
      const why = judge(cases[i].outcome, v.observed);
      return why === null ? [] : [`${v.name}: ${why}`];
    });
    expect(divergences).toEqual([]);
  });
}

// ---- SHOWN TO FAIL ----------------------------------------------------------------------------
//
// Each planted divergence goes into a COPY of the worker, asserted to have applied, and runs the
// same cases through the same `divergence`. One per comparison class that a copy of the glue can
// reach; the rest are held by hand below.
//
// AND TWO IN `bridge-qpdf.js`, the JS between the Rust policy and `qpdf.wasm` -- which #200's
// native-backed differential cannot see, and which is why #137 asked for this one. The first is
// #137's own example, `oh_set_array_item`'s arguments swapped: qpdf refuses it, so it arrives as
// a web refusal where native redacted. The second is SILENT: every stream written through
// `oh_replace_stream_data` loses its last byte -- whitespace, in every one logged, so the output
// means the same and nothing SHOULD reject it -- the web redaction passes its own verification,
// and only the digest comparison here sees it. Measured over the corpus on 2026-09-28: 113 cases
// in 56 documents. Dropping 20 bytes instead is refused on read-back, as it should be.

const WRITER = DOCUMENTS.find((d) => d.name === "tests/redaction/fixtures/producer-writer.pdf");
const BOMB = DOCUMENTS.find((d) => d.cases.some((c) => classOf(c.outcome) === "limit"));

for (const planted of [
  {
    // THE GLUE, not Rust: the region's first two sides swapped where the worker builds it.
    name: "a region handed to Rust with its left and top swapped",
    document: WRITER,
    from: "new wasm_bindgen.WebRegion(region.left, region.top, region.width, region.height)",
    to: "new wasm_bindgen.WebRegion(region.top, region.left, region.width, region.height)",
    finding: /a different document/,
  },
  {
    name: "a refusal reported under another rule's name",
    document: WRITER,
    from: "      message: reply.message,\n",
    to: '      message: reply.message.replace("[page-out-of-range]", "[another-rule]"),\n',
    finding: /refused by rule another-rule, not page-out-of-range/,
  },
  {
    name: "a ceiling reported at another stage",
    document: BOMB,
    from: "      stage: reply.stage,\n",
    to: '      stage: reply.stage === "prescan" ? "measured" : reply.stage,\n',
    finding: /stage measured, not prescan/,
  },
  {
    name: "the bridge's `oh_set_array_item` given its index and item swapped",
    document: WRITER,
    from: "qpdf()._qpdf_oh_set_array_item(data, oh, at, item);",
    to: "qpdf()._qpdf_oh_set_array_item(data, oh, item, at);",
    finding: /native redacted it; the web refused, Internal/,
    // NOT A BROKEN WORKER: a copy that throws on load or on any call produces the same finding.
    // qpdf's own refusal is the message, and the case the plant cannot reach -- a page out of
    // range, refused before any array is written -- still agrees with native (review).
    message: /qpdf reported an internal error/,
    unaffected: /\[page-out-of-range\]/,
  },
  {
    name: "the bridge dropping the last byte of every stream it rewrites",
    document: WRITER,
    from: "module._qpdf_oh_replace_stream_data(data, stream, buf, bytes.length, filter, decodeParms);",
    to: "module._qpdf_oh_replace_stream_data(data, stream, buf, Math.max(0, bytes.length - 1), filter, decodeParms);",
    finding: /a different document/,
  },
]) {
  test(`SHOWN TO FAIL: a copy of the worker with ${planted.name} diverges`, async ({ page }) => {
    const document = planted.document;
    if (document === undefined)
      throw new Error("the golden file no longer records the planted document");
    const found = documentBytes(document);
    if ("problem" in found) throw new Error(found.problem);
    await openHarness(page);
    const { applied } = await page.evaluate(
      (mutate) => window.burrowHarness.armRedaction({ mutate }),
      { from: planted.from, to: planted.to },
    );
    expect(applied, `the planted divergence did not apply: ${planted.name}`).toBe(true);

    const replies = await runCases(page, found.bytes, document.cases);
    const divergences = document.cases.flatMap((c, i) => divergence(c.outcome, replies[i]) ?? []);
    expect(divergences.join("\n"), "the differential passed a diverging worker").toMatch(
      planted.finding,
    );
    // NARROWED BY VALUE, not by `in`: the planted list is a union of object literals, and `in`
    // leaves the optional field `RegExp | undefined` under strict checking.
    const message = "message" in planted ? planted.message : undefined;
    if (message !== undefined) {
      const messages = replies.filter((r) => !r.ok).map((r) => r.message);
      expect(messages.join("\n"), "the refusal is not the planted defect's").toMatch(message);
    }
    const unaffected = "unaffected" in planted ? planted.unaffected : undefined;
    if (unaffected !== undefined) {
      const untouched = document.cases.flatMap((c, i) =>
        unaffected.test(c.outcome) ? [divergence(c.outcome, replies[i])] : [],
      );
      expect(
        untouched.length,
        "no case the plant cannot reach, so nothing tells it from a broken worker",
      ).toBeGreaterThan(0);
      expect(
        untouched,
        "a case the plant cannot reach diverged: the worker is broken, not planted",
      ).toEqual(untouched.map(() => null));
    }
  });
}

// THE COMPARISON ITSELF, by hand: each thing it compares, changed alone, is caught -- and the
// unchanged reply, beside all of them, is not.

const REFUSED: Reply = {
  ok: false,
  kind: "Unsupported",
  fatal: false,
  message: "unsupported: pdf redaction [optional-content]: …",
  pages: 0,
  limit: "",
  stage: "",
  requested: "0",
  allowed: "0",
  recycle: false,
  engineHeapBytes: "0",
  outputSha256: null,
} as Reply;
const RULE_OUTCOME = 'ERR Unsupported("pdf redaction [optional-content]: …")';
const LIMITED: Reply = {
  ...REFUSED,
  kind: "LimitExceeded",
  message: "limit exceeded",
  limit: "max_memory_bytes",
  stage: "prescan",
  requested: "1280000000",
  allowed: "1073741824",
};
const LIMIT_OUTCOME =
  'ERR LimitExceeded { limit: "max_memory_bytes", stage: Prescan, requested: 1280000000, allowed: 1073741824 }';

const OK_OUTCOME = `OK ${"a".repeat(64)} report ${createHash("sha256").update("Report {}").digest("hex")}`;
const REDACTED = {
  ...REFUSED,
  ok: true,
  kind: "",
  message: "",
  outputSha256: "a".repeat(64),
  report: "Report {}",
} as Reply;

test("the comparison passes a matching reply of every class", () => {
  expect(divergence(OK_OUTCOME, REDACTED)).toBeNull();
  expect(divergence(RULE_OUTCOME, REFUSED)).toBeNull();
  expect(divergence(LIMIT_OUTCOME, LIMITED)).toBeNull();
  expect(
    divergence("ERR PasswordRequired", { ...REFUSED, kind: "PasswordRequired", message: "" }),
  ).toBeNull();
  expect(
    divergence('ERR Malformed("qpdf: the document is damaged")', {
      ...REFUSED,
      kind: "Malformed",
      message: "x",
    }),
  ).toBeNull();
  expect(ruleOf(REFUSED.message)).toBe("optional-content");
});

for (const [name, outcome, reply, finding] of [
  [
    "another document",
    OK_OUTCOME,
    { ...REDACTED, outputSha256: "b".repeat(64) },
    /a different document$/,
  ],
  ["another report", OK_OUTCOME, { ...REDACTED, report: "Report { x }" }, /a different report$/],
  ["a refusal where native redacted", OK_OUTCOME, REFUSED, /the web refused/],
  ["a refusal of another kind", RULE_OUTCOME, { ...REFUSED, kind: "Malformed" }, /as Malformed/],
  [
    "a refusal under another rule",
    RULE_OUTCOME,
    { ...REFUSED, message: "[no-widths]" },
    /rule no-widths/,
  ],
  [
    "a rule where native had none",
    'ERR Malformed("qpdf: the document is damaged")',
    { ...REFUSED, kind: "Malformed" },
    /not \(none\)/,
  ],
  [
    "a redaction where native refused",
    RULE_OUTCOME,
    { ...REFUSED, ok: true },
    /the web redacted it/,
  ],
  ["another ceiling", LIMIT_OUTCOME, { ...LIMITED, limit: "max_pages" }, /limit max_pages/],
  ["another stage", LIMIT_OUTCOME, { ...LIMITED, stage: "measured" }, /stage measured/],
  ["another requested", LIMIT_OUTCOME, { ...LIMITED, requested: "1" }, /requested 1,/],
  ["another allowed", LIMIT_OUTCOME, { ...LIMITED, allowed: "1" }, /allowed 1,? /],
  [
    "a stage STAGES does not name",
    LIMIT_OUTCOME.replace("Prescan", "Elsewhere"),
    LIMITED,
    /STAGES does not name/,
  ],
] as const) {
  test(`SHOWN TO FAIL, by hand: ${name}`, () => {
    expect(divergence(outcome, reply as Reply) ?? "").toMatch(finding);
  });
}

// ---- THE LEDGER ------------------------------------------------------------------------------
//
// By hand first: `reconcile` over the real golden file, with one thing wrong at a time, and the
// near-miss beside each -- a drop that IS declared passes, and is reported as a skip by name.

const EXPECTED = goldenCases();
const EVERY = [...EXPECTED.keys()];
const FIRST = EVERY[0];
const FIRST_DOCUMENT = DOCUMENTS[0].name;

/**
 * An observation `judge` accepts for `outcome`, built from the outcome. The probe that expects no
 * problems over every case is what holds this to `judge`: a wrong one fails it.
 */
function agreeingWith(outcome: string): Observed {
  const none: Observed = {
    ok: false,
    kind: "",
    rule: null,
    limit: "",
    stage: "",
    requested: "0",
    allowed: "0",
    outputSha256: null,
    reportSha256: null,
  };
  if (outcome.startsWith("OK ")) {
    const [, document, , report] = outcome.split(" ");
    return { ...none, ok: true, outputSha256: document, reportSha256: report };
  }
  const kind = /^ERR ([A-Za-z]+)/.exec(outcome)?.[1] ?? "";
  if (kind !== "LimitExceeded") return { ...none, kind, rule: ruleOf(outcome) };
  return {
    ...none,
    kind,
    limit: /limit: "([a-z_]+)"/.exec(outcome)?.[1] ?? "",
    stage: STAGES[/stage: ([A-Za-z]+)/.exec(outcome)?.[1] ?? ""] ?? "",
    requested: /requested: (\d+)/.exec(outcome)?.[1] ?? "",
    allowed: /allowed: (\d+)/.exec(outcome)?.[1] ?? "",
  };
}

const agreeing = (names: string[]): Recorded[] =>
  names.map((name) => ({ name, observed: agreeingWith(EXPECTED.get(name)?.outcome ?? "") }));

test("SHOWN TO FAIL, by hand: the ledger refuses a case compared and diverged, even parked", () => {
  const recorded = agreeing(EVERY).map((r) =>
    r.name === FIRST ? { ...r, observed: { ...r.observed, kind: "Planted", ok: false } } : r,
  );
  const { problems, summary } = reconcile(EXPECTED, recorded, []);
  expect(problems).toHaveLength(1);
  expect(problems[0]).toContain(`compared, and diverged: ${FIRST}: `);
  expect(summary).toContain(`${EXPECTED.size} compared (`);
  expect(summary).toContain("1 diverged");
});

test("SHOWN TO FAIL, by hand: the ledger judges by name, not by what the test concluded", () => {
  // Every case's observation swapped with the next one's: each name is right and each reply real,
  // but bound to another case -- what a verdict computed against the wrong case looks like.
  const recorded = agreeing(EVERY);
  const shifted = recorded.map((r, i) => ({
    name: r.name,
    observed: recorded[(i + 1) % recorded.length].observed,
  }));
  const { problems } = reconcile(EXPECTED, shifted, []);
  expect(problems.join("\n")).toContain(`compared, and diverged: ${FIRST}: `);
});

test("SHOWN TO FAIL, by hand: the ledger refuses a record that is not an observation", () => {
  const recorded = agreeing(EVERY).map((r) =>
    r.name === FIRST ? { name: r.name, observed: { agreed: true } as unknown as Observed } : r,
  );
  const { problems } = reconcile(EXPECTED, recorded, []);
  expect(problems).toEqual([
    `compared, and diverged: ${FIRST}: a record that is not an observation`,
  ]);
});

test("the ledger passes every case compared once, and reports the count against the golden file's", () => {
  const { problems, summary } = reconcile(EXPECTED, agreeing(EVERY), []);
  expect(problems).toEqual([]);
  expect(summary).toMatch(
    new RegExp(`^${EXPECTED.size} cases in the golden file: ${EXPECTED.size} compared \\(`),
  );
  expect(summary).toMatch(/, 0 skipped$/);
});

test("the ledger passes a declared skip, and names it with its reason", () => {
  const { problems, summary } = reconcile(EXPECTED, agreeing(EVERY.slice(1)), [
    { document: FIRST_DOCUMENT, case: FIRST, reason: "a reason" },
  ]);
  expect(problems).toEqual([]);
  expect(summary).toContain(`${EXPECTED.size - 1} compared`);
  expect(summary).toContain(`1 skipped\n  skipped 1: ${FIRST} -- a reason`);
});

for (const [name, compared, skips, finding] of [
  [
    "a case dropped silently",
    EVERY.slice(1),
    [],
    `not compared, and not declared skipped: ${FIRST}`,
  ],
  [
    "a document dropped silently",
    EVERY.filter((k) => !k.startsWith(`${FIRST_DOCUMENT} page `)),
    [],
    `not compared, and not declared skipped: ${FIRST}`,
  ],
  [
    "a skip with no reason",
    EVERY.slice(1),
    [{ document: FIRST_DOCUMENT, case: FIRST, reason: " " }],
    `declared skipped with no reason: ${FIRST}`,
  ],
  [
    "a skip naming a case the golden file does not record",
    EVERY,
    [
      {
        document: FIRST_DOCUMENT,
        case: `${FIRST_DOCUMENT} page 99 covering 99, band`,
        reason: "r",
      },
    ],
    "the golden file records no such case",
  ],
  [
    "a skip naming a document the golden file does not record",
    EVERY,
    [{ document: "tests/redaction/nowhere.pdf", reason: "r" }],
    "the golden file records no such case: tests/redaction/nowhere.pdf (every case)",
  ],
  [
    "a case declared skipped and compared",
    EVERY,
    [{ document: FIRST_DOCUMENT, case: FIRST, reason: "r" }],
    `declared skipped, and compared: ${FIRST}`,
  ],
  ["a case compared twice", [...EVERY, FIRST], [], `compared 2 times: ${FIRST}`],
  ["a case not in the golden file", [...EVERY, "nowhere"], [], "not in the golden file: nowhere"],
] as const) {
  test(`SHOWN TO FAIL, by hand: the ledger refuses ${name}`, () => {
    const { problems } = reconcile(EXPECTED, agreeing([...compared]), skips);
    expect(problems.join("\n")).toContain(finding);
  });
}

// LAST IN THE FILE, so in each browser it runs after every document test above it.
test("the ledger: every case in the golden file was compared here and agreed, or is declared skipped", () => {
  let recorded: Recorded[] = [];
  try {
    recorded = readFileSync(ledgerPath(test.info()), "utf8")
      .split("\n")
      .filter((l) => l !== "")
      .map((l) => JSON.parse(l) as Recorded);
  } catch {
    // No ledger at all: nothing was compared, and `reconcile` names every case as not compared.
  }
  const { problems, summary } = reconcile(EXPECTED, recorded, DECLARED_SKIPS);
  test.info().annotations.push({ type: "ledger", description: summary });
  console.log(`redaction differential [${test.info().project.name}]: ${summary}`);
  // THE FIRST TWENTY, then the count: a run that dropped everything names 463 cases otherwise.
  const shown = problems.slice(0, 20);
  if (problems.length > shown.length) shown.push(`... and ${problems.length - shown.length} more`);
  expect(shown, summary).toEqual([]);
});
