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
// golden file recorded, FAILS BY NAME; nothing is skipped, because a differential that quietly
// compared fewer documents would read exactly like one that compared them all.
//
// WHAT IS COMPARED is `redaction-differential.ts`' header: both digests of a redaction, and a
// refusal's typed kind and rule name -- never its prose.

import { createHash } from "node:crypto";

import { expect, test } from "@playwright/test";

import type { Reply } from "../src/host/harness-api";
import { openHarness } from "./harness";
import {
  type Case,
  type Class,
  classOf,
  divergence,
  documentBytes,
  golden,
  ruleOf,
} from "./redaction-differential";

const DOCUMENTS = golden();

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
  // THE COUNTS, AND WHAT THEY SHOULD BE, both from the golden file: a run over fewer documents
  // or cases reads as fewer, not as success.
  const cases = DOCUMENTS.flatMap((d) => d.cases);
  const problems = DOCUMENTS.map(documentBytes).flatMap((r) => ("problem" in r ? [r.problem] : []));
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

for (const document of DOCUMENTS) {
  test(`every recorded redaction of ${document.name} is the native one`, async ({ page }) => {
    const found = documentBytes(document);
    if ("problem" in found) throw new Error(found.problem);
    await openHarness(page);
    const replies = await runCases(page, found.bytes, document.cases);
    expect(replies, "a case produced no reply").toHaveLength(document.cases.length);

    const divergences = document.cases.flatMap((c, i) => {
      const why = divergence(c.outcome, replies[i]);
      const covered = c.covered.join(",");
      return why === null ? [] : [`page ${c.page} covering ${covered}, ${c.label}: ${why}`];
    });
    expect(divergences).toEqual([]);
  });
}

// ---- SHOWN TO FAIL ----------------------------------------------------------------------------
//
// Each planted divergence goes into a COPY of the worker, asserted to have applied, and runs the
// same cases through the same `divergence`. One per comparison class that a copy of the glue can
// reach; the rest are held by hand below.

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
