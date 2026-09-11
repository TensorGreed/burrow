// The two numbers this PR was not allowed to guess at: what a respawn costs, and how far an
// engine heap actually grows.
//
// WHY THESE ARE TESTS RATHER THAN A ONE-OFF SCRIPT
//
// Both feed decisions recorded in ADR 0015 — the heap threshold in
// `core/burrow-engines/src/web/recycle.rs`, and whether pre-compiled `WebAssembly.Module`s
// need passing to new workers. A measurement that decides something should be repeatable by
// whoever reads the decision, in the same three browsers, without reconstructing a rig.
//
// The assertions are deliberately loose. A CI runner is not a benchmark and a tight bound here
// would be a flake generator; what is asserted is only that the numbers are in the range the
// decision assumed. The numbers themselves are written to `test-results/` and logged.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";

const here = dirname(fileURLToPath(import.meta.url));
const conformance = resolve(here, "../../../tests/conformance");

function fixture(name: string): number[] {
  return Array.from(readFileSync(join(conformance, "fixtures", name)));
}

const MIB = 1024 * 1024;

test("respawn cost, including compiling the engines", async ({ page }, testInfo) => {
  await openHarness(page);

  // Five cold respawns. Each one re-fetches 6.5 MB of WebAssembly (from a `no-store` server,
  // so no HTTP cache is hiding the fetch) and compiles all three modules.
  const samples: number[] = [];
  for (let i = 0; i < 5; i += 1) {
    const elapsed = await page.evaluate(async () => {
      window.burrowHarness.discardWorker();
      const started = performance.now();
      await window.burrowHarness.ready();
      return performance.now() - started;
    });
    samples.push(Math.round(elapsed));
  }

  const median = [...samples].sort((a, b) => a - b)[Math.floor(samples.length / 2)];
  const report = { browser: testInfo.project.name, samples, medianMs: median };
  writeFileSync(
    join(
      testInfo.project.outputDir ?? "test-results",
      `respawn-cost.${testInfo.project.name}.json`,
    ),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  testInfo.annotations.push({ type: "respawn-cost", description: JSON.stringify(report) });
  console.log(`respawn cost [${testInfo.project.name}]: ${JSON.stringify(report)}`);

  // THE DECISION THIS FEEDS. ADR 0014 §1a generates the engine manifest INTO the worker bundle
  // because `pdfium.js` starts instantiating as it is parsed — so handing a new worker
  // pre-compiled modules by `postMessage` cannot arrive in time, and moving the
  // integrity-pinned fetch to the page would have to be shown to keep both SRI and the
  // in-worker policy guard intact. That re-derivation is only worth doing if a respawn is
  // expensive enough to matter.
  //
  // The bound below is what "does not matter" means here: a respawn happens after a crash, a
  // watchdog kill, or a recycle, all of which are rare and none of which is on a hot path. If
  // this ever fails, ADR 0014 §1a needs revisiting rather than the bound relaxing.
  expect(median, `median respawn ${median}ms`).toBeLessThan(15_000);
});

test("engine heap growth across the corpus, and what the bombs cost", async ({
  page,
}, testInfo) => {
  await openHarness(page);

  const files = [
    "blank-1page.pdf",
    "pages-10.pdf",
    "pages-137.pdf",
    "truncated.pdf",
    "not-a-pdf.bin",
    "no-pages.pdf",
    "encrypted.pdf",
    // THE ONE THAT MATTERS. 330 KB declaring a twenty-million-entry cross-reference stream.
    // On native it drives PDFium to allocate ~1.2 GB; the pre-scan refuses it first, and this
    // records what the heap does anyway.
    "xref-bomb.pdf",
  ];

  const rows: { file: string; op: string; kind: string; pdfiumMiB: number; qpdfMiB: number }[] = [];

  for (const name of files) {
    for (const op of ["page_count", "structure_check"] as const) {
      const reply = await page.evaluate(
        ([operation, bytes, limits]) =>
          window.burrowHarness.run(
            operation as "page_count" | "structure_check",
            bytes as number[],
            // Ceilings high enough that nothing is refused for a limit reason before the
            // engine has had a chance to grow. Measuring under a limit that rejects the file
            // would measure the limit, not the file.
            //
            // Passed in as an argument rather than read from the enclosing scope: the callback
            // is serialised and evaluated in the BROWSER, where `MIB` does not exist. The
            // first version closed over it and failed with `ReferenceError: MIB is not
            // defined` — which is easy to forget, and which TypeScript cannot catch.
            { limits },
          ),
        [op, fixture(name), { maxMemoryBytes: 4 * 1024 * MIB, maxInputBytes: 512 * MIB }] as const,
      );
      rows.push({
        file: name,
        op,
        kind: reply.ok ? "ok" : reply.kind,
        pdfiumMiB: Math.round((Number(reply.pdfiumHeapBytes) / MIB) * 10) / 10,
        qpdfMiB: Math.round((Number(reply.qpdfHeapBytes) / MIB) * 10) / 10,
      });
    }
  }

  const peakPdfium = Math.max(...rows.map((r) => r.pdfiumMiB));
  const peakQpdf = Math.max(...rows.map((r) => r.qpdfMiB));
  const report = { browser: testInfo.project.name, peakPdfium, peakQpdf, rows };
  writeFileSync(
    join(testInfo.project.outputDir, `heap-growth.${testInfo.project.name}.json`),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  console.log(
    `heap growth [${testInfo.project.name}]: peak pdfium ${peakPdfium} MiB, qpdf ${peakQpdf} MiB`,
  );
  for (const row of rows) {
    console.log(
      `  ${row.file.padEnd(18)} ${row.op.padEnd(16)} ${row.kind.padEnd(16)} ` +
        `pdfium ${String(row.pdfiumMiB).padStart(7)} MiB  qpdf ${String(row.qpdfMiB).padStart(7)} MiB`,
    );
  }

  // A heap that never grew would mean the reading is not wired up — the same failure the
  // native side hit in 4a-i, where `heap_bytes` was declared and called from nowhere.
  expect(peakPdfium, "an engine heap that never grows means the reading is dead").toBeGreaterThan(
    0,
  );
  expect(peakQpdf).toBeGreaterThan(0);

  // THE TWO POPULATIONS ARE MEASURED SEPARATELY, because the whole design rests on them being
  // far apart. Measured in Chromium, M1 PR 4a-ii:
  //
  //   ordinary corpus work, every fixture and both engines   pdfium 17.9 MiB   qpdf 16 MiB
  //   xref-bomb.pdf via page_count, ceiling raised to 4 GiB  pdfium 1900.7 MiB
  //   xref-bomb.pdf via structure_check, same ceiling                          qpdf 513 MiB
  //
  // Two orders of magnitude between them, with nothing in between. That is what makes the
  // 512 MiB floor in `recycle.rs` a threshold rather than a guess: any value from ~64 MiB to
  // ~512 MiB separates the populations identically.
  const ordinary = rows.filter((row) => row.file !== "xref-bomb.pdf");
  const ordinaryPeak = Math.max(...ordinary.flatMap((r) => [r.pdfiumMiB, r.qpdfMiB]));
  expect(
    ordinaryPeak,
    "ordinary corpus work must sit far below the recycling floor, or recycling would be " +
      "discarding healthy workers on every operation",
  ).toBeLessThan(64);

  // And the floor must be REACHABLE by a real file, or recycling is a mechanism that never
  // fires. The bomb is the file that reaches it — and only when a caller has raised
  // `max_memory_bytes` above what the pre-scan would otherwise refuse.
  const bomb = Math.max(
    ...rows.filter((r) => r.file === "xref-bomb.pdf").flatMap((r) => [r.pdfiumMiB, r.qpdfMiB]),
  );
  expect(
    bomb,
    "the recycling threshold must be reachable, or it is protection that never runs",
  ).toBeGreaterThan(512);
});
