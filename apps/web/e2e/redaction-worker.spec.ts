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

/** One golden line, by input, page and region label. Refuses if it is not there exactly once. */
function pinned(input: string, page: number, label: string): Pinned {
  const lines = readFileSync(join(repo, "tests/redaction/outcomes.tsv"), "utf8")
    .split("\n")
    .filter((line) => line.startsWith("CASE\t"))
    .map((line) => line.split("\t"))
    .filter(([, name, p, , l]) => name === input && Number(p) === page && l === label);
  expect(lines, `${input} page ${page} ${label} is pinned exactly once`).toHaveLength(1);
  const [, , , covered, , outcome] = lines[0];
  return { input, page, covered: covered.split(",").map(Number), label, outcome };
}

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

const WRITER = "tests/redaction/fixtures/producer-writer.pdf";

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

test("a malformed request is refused before the document is read", async ({ page }) => {
  await openHarness(page);
  const bytes = Array.from(readFileSync(join(repo, WRITER)));
  const zero = await page.evaluate(
    (b) =>
      window.burrowHarness.redactDocument("w.pdf", new Uint8Array(b), 0, [0], {
        left: 0,
        top: 0,
        width: 10,
        height: 10,
      }),
    bytes,
  );
  // Zero is Rust's to refuse, and it says why: pages are numbered from 1.
  expect([zero.ok, zero.kind]).toEqual([false, "InvalidArgument"]);
  expect(zero.message).toContain("numbered from 1");

  const fractional = await page.evaluate(
    (b) =>
      window.burrowHarness.redactDocument("w.pdf", new Uint8Array(b), 1.5, [1], {
        left: 0,
        top: 0,
        width: 10,
        height: 10,
      }),
    bytes,
  );
  expect([fractional.ok, fractional.kind]).toEqual([false, "InvalidArgument"]);
});

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
