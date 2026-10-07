// Redaction's own worker, driven end to end in all three browsers (#137).
//
// WHAT THIS IS, AND WHAT IT IS NOT. It proves the wiring: the third bundle is staged into the
// harness build, its worker starts, `redact` reaches the Rust entry point, and what comes back is
// the outcome the native golden file pins -- byte for byte, report included -- for a handful of
// cases. It is NOT #137's browser differential, which runs every case in
// `tests/redaction/outcomes.tsv` and plants a divergence in the glue to show it can fail. This
// is the smoke test that has to pass before that one means anything.
//
// WHY THE CASES ARE READ FROM THE GOLDEN FILE rather than restated here: a digest typed into a
// test is a second copy of the pin, and the day the corpus is re-blessed the two disagree for a
// reason nobody is looking for.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";
import { mark, since } from "./request-log";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");

/** The regions `core/burrow-ops/tests/redaction_outcomes.rs` names, by label. */
const REGIONS: Record<string, { left: number; top: number; width: number; height: number }> = {
  whole: { left: 0, top: 0, width: 5000, height: 5000 },
  band: { left: 0, top: 300, width: 5000, height: 120 },
  pdfbuild: { left: 30, top: 68, width: 340, height: 44 },
};

interface Pinned {
  input: string;
  /** 0-based, as the golden file records it. */
  page: number;
  covered: number[];
  label: string;
  outcome: string;
}

const GOLDEN = readFileSync(join(repo, "tests/redaction/outcomes.tsv"), "utf8").split("\n");

/**
 * One golden line, by input, page, region label and covered pages (0-based, comma-joined as the
 * golden file writes them). Refuses if it is not there exactly once.
 */
function pinned(input: string, page: number, label: string, covered = String(page)): Pinned {
  const lines = GOLDEN.filter((line) => line.startsWith("CASE\t"))
    .map((line) => line.split("\t"))
    .filter(
      ([, name, p, c, l]) => name === input && Number(p) === page && c === covered && l === label,
    );
  expect(
    lines,
    `${input} page ${page} ${label} covering ${covered} is pinned exactly once`,
  ).toHaveLength(1);
  const [, , , pages, , outcome] = lines[0];
  return { input, page, covered: pages.split(",").map(Number), label, outcome };
}

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

const WRITER = "tests/redaction/fixtures/producer-writer.pdf";

/**
 * The generators' builders for the two fixtures these cases use, by file name: the module, and
 * the expression that yields the builder from it.
 */
const BUILDERS: Record<string, [string, string]> = {
  "09-actualtext.pdf": ["make-redaction-fixtures.py", "m.ch09_actualtext"],
  "nearmiss-oc-on-another-page.pdf": [
    "make-evasion-fixtures.py",
    'm.BUILDERS["nearmiss-oc-on-another-page"]',
  ],
};

/**
 * A GENERATED redaction fixture's bytes, held to the digest `outcomes.tsv` records for it.
 *
 * `tests/redaction/generated/` is gitignored, and no COMMITTED fixture produces a non-zero
 * disclosure count -- scanned, 2026-09-26. So the fixture is built here by calling its
 * generator's BUILDER directly, and nothing else: the generators' `main` also validates every
 * fixture with a native `qpdf --check` and needs a native `cjpeg` for channel 20, neither of
 * which CI's `web` job has (both reviews of #137 caught the first version, which ran `main`).
 * The bytes come back on stdout; nothing is written anywhere, and `-B` keeps Python from
 * leaving bytecode in `tools/`.
 *
 * Every fixture is checked against its `INPUT` line before use: a builder that changed would
 * otherwise be compared against the pin for a different document, and fail for a reason
 * nobody is looking for.
 */
function generated(name: string): number[] {
  const [tool, builder] = BUILDERS[name];
  const bytes = execFileSync(
    "python3",
    [
      "-B",
      "-c",
      [
        "import importlib.util, sys",
        `spec = importlib.util.spec_from_file_location("gen", ${JSON.stringify(join(repo, "tools", tool))})`,
        "m = importlib.util.module_from_spec(spec)",
        `sys.path.insert(0, ${JSON.stringify(join(repo, "tools"))})`,
        "spec.loader.exec_module(m)",
        `sys.stdout.buffer.write(${builder}())`,
      ].join("\n"),
    ],
    { maxBuffer: 16 * 1024 * 1024 },
  );
  const digest = createHash("sha256").update(bytes).digest("hex");
  expect(
    GOLDEN.includes(`INPUT\ttests/redaction/generated/${name}\t${digest}`),
    `${name} is not the document outcomes.tsv pins; the builder changed, so re-bless or fix it`,
  ).toBe(true);
  return Array.from(bytes);
}

for (const label of ["whole", "band", "pdfbuild"]) {
  test(`a redaction through the worker is the pinned one, byte for byte: ${label}`, async ({
    page,
  }) => {
    const pin = pinned(WRITER, 0, label);
    const [status, digest, word, reportDigest] = pin.outcome.split(" ");
    expect([status, word], "the pin is a success with a report").toEqual(["OK", "report"]);

    await openHarness(page);
    const reply = await page.evaluate(
      ({ bytes, pageNumber, covered, region }) =>
        window.burrowHarness.redactDocument(
          "producer-writer.pdf",
          new Uint8Array(bytes),
          pageNumber,
          covered,
          region,
        ),
      {
        bytes: Array.from(readFileSync(join(repo, WRITER))),
        pageNumber: pin.page + 1,
        covered: pin.covered.map((n) => n + 1),
        region: REGIONS[label],
      },
    );

    expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(true);
    expect(reply.outputSha256, "the redacted document is not the pinned one").toBe(digest);
    expect(sha256(reply.report ?? ""), "the report is not the pinned one").toBe(reportDigest);
  });
}

test("a page past the end is refused as the native operation refuses it", async ({ page }) => {
  const pin = pinned(WRITER, 1, "whole");
  expect(pin.outcome).toMatch(/^ERR InvalidArgument\(".*\[page-out-of-range\]/);

  await openHarness(page);
  const reply = await page.evaluate(
    (bytes) =>
      window.burrowHarness.redactDocument("producer-writer.pdf", new Uint8Array(bytes), 2, [2], {
        left: 0,
        top: 0,
        width: 5000,
        height: 5000,
      }),
    Array.from(readFileSync(join(repo, WRITER))),
  );
  expect(reply.ok).toBe(false);
  expect(reply.kind).toBe("InvalidArgument");
  expect(reply.message).toContain("[page-out-of-range]");
  expect(reply.outputSha256, "a refusal carries no document").toBeNull();
});

/** Redact `WRITER` with arguments a test chose, through the real worker. */
async function redactWriter(
  page: import("@playwright/test").Page,
  pageNumber: unknown,
  covered: unknown,
  region: unknown,
  limits?: Record<string, number>,
) {
  return page.evaluate(
    ({ b, pageNumber, covered, region, limits }) =>
      window.burrowHarness.redactDocument(
        "w.pdf",
        new Uint8Array(b),
        pageNumber as number,
        covered as number[],
        region as { left: number; top: number; width: number; height: number },
        { limits },
      ),
    { b: Array.from(readFileSync(join(repo, WRITER))), pageNumber, covered, region, limits },
  );
}

const TEN = { left: 0, top: 0, width: 10, height: 10 };

test("malformed arguments are refused by the worker, before the document is read", async ({
  page,
}) => {
  await openHarness(page);
  // EACH GUARD IN `redact-main.js` BY ITS OWN MESSAGE, so a refusal from a different layer --
  // Rust truncating 1.5 to 1 and refusing something else, or accepting it -- cannot pass as
  // this one. All three survived both reviews' mutations until these existed.
  const fractionalPage = await redactWriter(page, 1.5, [1], TEN);
  expect([fractionalPage.ok, fractionalPage.kind]).toEqual([false, "InvalidArgument"]);
  expect(fractionalPage.message).toBe("a page number is not a whole number in range");

  const fractionalCovered = await redactWriter(page, 1, [1, 2.5], TEN);
  expect([fractionalCovered.ok, fractionalCovered.kind]).toEqual([false, "InvalidArgument"]);
  expect(fractionalCovered.message).toBe("a page number is not a whole number in range");

  // `null` would reach an `f64` as 0 through wasm-bindgen's ToNumber, silently moving the region.
  const nullSide = await redactWriter(page, 1, [1], { ...TEN, left: null });
  expect([nullSide.ok, nullSide.kind]).toEqual([false, "InvalidArgument"]);
  expect(nullSide.message).toBe("a region is four numbers: left, top, width and height");
});

test("page zero is refused by Rust, which says why", async ({ page }) => {
  // AFTER the bytes are read, unlike the refusals above: zero is a whole number in range for
  // `u32`, so the worker hands it over and the binding refuses it by name.
  await openHarness(page);
  const zero = await redactWriter(page, 0, [0], TEN);
  expect([zero.ok, zero.kind]).toEqual([false, "InvalidArgument"]);
  expect(zero.message).toContain("numbered from 1");
});

test("a document over the input ceiling is refused before it is read", async ({ page }) => {
  // THE BUDGET, checked in Rust against the Blob's size before `arrayBuffer()` -- the base
  // bundle's #51 ordering, which review found untested for this bundle.
  await openHarness(page);
  const size = readFileSync(join(repo, WRITER)).length;
  const refused = await redactWriter(page, 1, [1], TEN, { maxInputBytes: size - 1 });
  expect([refused.ok, refused.kind, refused.limit]).toEqual([
    false,
    "LimitExceeded",
    "max_input_bytes",
  ]);
});

/** The report's own statement of each count, read from the Debug text `outcomes.tsv` pins. */
function statedCounts(report: string): { retained: number; dropped: number } {
  const dropped = /dropped_carried_text: (\d+)/.exec(report);
  expect(dropped, "the report does not state dropped_carried_text").not.toBeNull();
  return { retained: (report.match(/cut: false/g) ?? []).length, dropped: Number(dropped![1]) };
}

for (const [name, label, covered, expectRetained, expectDropped] of [
  // One font kept because page 2 still uses it: ADR 0029 §7's disclosure. (Was
  // evade-widget-on-another-page until #125 made it refuse [acroform-field]; this twin
  // is a near-miss that still redacts, two pages sharing /Helv.)
  ["nearmiss-oc-on-another-page.pdf", "whole", "0", 1, 0],
  // One `/ActualText` whose carried text went with the redaction.
  ["09-actualtext.pdf", "whole", "0", 0, 1],
] as const) {
  test(`the disclosure counts are the pinned report's, and not zero: ${name}`, async ({ page }) => {
    const pin = pinned(`tests/redaction/generated/${name}`, 0, label, covered);
    const [status, digest, word, reportDigest] = pin.outcome.split(" ");
    expect([status, word]).toEqual(["OK", "report"]);

    await openHarness(page);
    const reply = await page.evaluate(
      ({ bytes, covered, region }) =>
        window.burrowHarness.redactDocument("g.pdf", new Uint8Array(bytes), 1, covered, region),
      {
        bytes: generated(name),
        covered: pin.covered.map((n) => n + 1),
        region: REGIONS[label],
      },
    );
    expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(true);
    expect(reply.outputSha256).toBe(digest);
    // THE REPORT IS THE PINNED ONE, so its stated counts are pinned -- and the reply's two
    // numbers must be those, and non-zero where the case says so. Different values in each
    // case, so a swap of the two fields cannot pass both.
    expect(sha256(reply.report ?? ""), "the report is not the pinned one").toBe(reportDigest);
    const stated = statedCounts(reply.report ?? "");
    expect([stated.retained, stated.dropped]).toEqual([expectRetained, expectDropped]);
    expect([reply.retainedFonts, reply.droppedCarriedText]).toEqual([
      expectRetained,
      expectDropped,
    ]);
  });
}

test("redaction's bundle is fetched only when something asks for a redaction", async ({ page }) => {
  const marker = "redaction-worker/lazy";
  await mark(marker);
  await openHarness(page);
  await page.evaluate(() => window.burrowHarness.run("page_count", Array.from([37, 80, 68, 70])));
  const before = since(marker).map((entry) => entry.url);
  expect(
    before.filter((url) => /redact/.test(url)),
    "the harness fetched redaction's bundle before anything asked for a redaction",
  ).toEqual([]);
  // THE CONTROL: the log is not empty, so the empty filter above measured something.
  expect(before.length, "nothing was logged at all").toBeGreaterThan(0);

  await page.evaluate(
    (b) =>
      window.burrowHarness.redactDocument("w.pdf", new Uint8Array(b), 1, [1], {
        left: 0,
        top: 0,
        width: 5000,
        height: 5000,
      }),
    Array.from(readFileSync(join(repo, WRITER))),
  );
  const after = since(marker).map((entry) => entry.url);
  expect(
    after.filter((url) => /burrow-redact-worker|burrow_wasm_redact_bg/.test(url)),
  ).toHaveLength(2);
});
