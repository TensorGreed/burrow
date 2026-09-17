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

  // Five cold respawns. Each one re-fetches the whole engine payload (from a `no-store`
  // server, so no HTTP cache is hiding the fetch) and compiles every module. That was 6.8 MB
  // across three; since spike 0004 it is about 1.8 MB across two.
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
  // because the Emscripten glue starts instantiating as it is parsed — so handing a new worker
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

test("the slowest LEGITIMATE operation in the corpus, which sets the watchdog budget", async ({
  page,
}, testInfo) => {
  // THE NUMBER THE WATCHDOG IS DERIVED FROM, measured rather than chosen.
  //
  // `LIMITS.maxDurationMs` was 120 s, so a crafted file that hangs an engine call froze the
  // tab for two minutes before failing. That was accepted as a residual for a LOCAL build
  // (`docs/security/exposure-2026-09-14-qpdf-uaf.md`), and on a public URL it is a different
  // question: a minute-long freeze reads as a broken site, and most people close the tab long
  // before the error lands. So the budget comes down.
  //
  // IT CANNOT COME DOWN TO A GUESS. A budget under the slowest real operation turns a working
  // tool into one that refuses honest documents with `LimitExceeded`, which is worse than the
  // freeze: the freeze ends in a correct answer, and a premature refusal is a wrong one. So
  // this walks the corpus, times every operation on every fixture, and reports the slowest
  // that SUCCEEDS -- failures are excluded deliberately, because a file that is refused is
  // not work anybody is waiting on.
  //
  // The assertion is loose on purpose, for the reason this file's header gives: a CI runner is
  // not a benchmark. What is asserted is that the slowest honest operation still sits far
  // enough under the budget that the budget is about hangs rather than about work.
  await openHarness(page);

  const files = [
    "blank-1page.pdf",
    "pages-10.pdf",
    // THE BIGGEST HONEST DOCUMENT IN THE CORPUS, and the one that decides this number.
    "pages-137.pdf",
    "inherited-rotation-6page.pdf",
    "mixed-rotation-4page.pdf",
    // Large and legitimate: 204 KB of object streams. It is a bomb by construction and it is
    // also a file qpdf reads, so whichever way it lands it belongs in the sample.
    "objstm-bomb.pdf",
  ];

  const rows: { file: string; op: string; ok: boolean; ms: number }[] = [];

  for (const name of files) {
    for (const op of ["page_count", "structure_check"] as const) {
      const measured = await page.evaluate(
        async ([operation, bytes, limits]) => {
          const started = performance.now();
          const reply = await window.burrowHarness.run(
            operation as "page_count" | "structure_check",
            bytes as number[],
            { limits },
          );
          return { ms: performance.now() - started, ok: reply.ok };
        },
        [op, fixture(name), { maxMemoryBytes: 4 * 1024 * MIB, maxInputBytes: 512 * MIB }] as const,
      );
      rows.push({ file: name, op, ok: measured.ok, ms: Math.round(measured.ms) });
    }
  }

  const succeeded = rows.filter((r) => r.ok);
  expect(
    succeeded.length,
    "no operation in the sample succeeded, so the slowest one is not a measurement of work",
  ).toBeGreaterThan(4);

  const slowest = succeeded.reduce((worst, row) => (row.ms > worst.ms ? row : worst));
  const table = rows
    .map((r) => `${r.file}\t${r.op}\t${r.ok ? "ok" : "refused"}\t${r.ms} ms`)
    .join("\n");
  writeFileSync(
    join(testInfo.project.outputDir ?? "test-results", "watchdog-budget.txt"),
    `slowest successful: ${slowest.file} ${slowest.op} ${slowest.ms} ms\n\n${table}\n`,
  );
  console.log(`slowest successful operation: ${slowest.file} ${slowest.op} — ${slowest.ms} ms`);
  console.log(table);

  // THE BUDGET IS 12 s. This asserts the headroom that number was chosen for: the slowest
  // honest operation must finish inside a quarter of it, so an ordinary document on a slower
  // machine than this runner still has three times the margin it needs. If this ever fails,
  // the answer is to re-derive the budget from the new measurement and amend ADR 0015 --
  // never to raise it quietly so the suite goes green.
  expect(
    slowest.ms,
    `the slowest honest operation (${slowest.file} ${slowest.op}) is ${slowest.ms} ms, which is ` +
      `not comfortably inside the 12 s watchdog budget. Re-derive the budget and amend ADR 0015.`,
  ).toBeLessThan(3_000);
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

  const rows: { file: string; op: string; kind: string; qpdfMiB: number }[] = [];

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
        qpdfMiB: Math.round((Number(reply.engineHeapBytes) / MIB) * 10) / 10,
      });
    }
  }

  // ONE ENGINE SINCE SPIKE 0004. This reported two columns because the web held two modules;
  // PDFium is no longer in the payload, so a `peakPdfium` column would be a zero that a
  // reader could not tell from "allocated nothing".
  const peakQpdf = Math.max(...rows.map((r) => r.qpdfMiB));
  const report = { browser: testInfo.project.name, peakQpdf, rows };
  writeFileSync(
    join(testInfo.project.outputDir, `heap-growth.${testInfo.project.name}.json`),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  console.log(`heap growth [${testInfo.project.name}]: peak qpdf ${peakQpdf} MiB`);
  for (const row of rows) {
    console.log(
      `  ${row.file.padEnd(18)} ${row.op.padEnd(16)} ${row.kind.padEnd(16)} ` +
        `qpdf ${String(row.qpdfMiB).padStart(7)} MiB`,
    );
  }

  // ---- the baseline, and the constant that rests on it ---------------------------
  //
  // `MIN_CONVERGING_MEMORY_BYTES` (64 MiB) exists because a worker that has done any real
  // work occupies a baseline per engine, and a `max_memory_bytes` under roughly twice that
  // puts the recycling threshold below the baseline — so every operation costs a respawn and
  // the heap never gets back under the line.
  //
  // Until PR 4b that baseline was ONE MEASUREMENT, taken once, written into a doc comment.
  // This is the check that it stays true: the floor is read from Rust (not copied into this
  // file, which would be a second definition free to drift), and the baseline is whatever the
  // engines actually report on a fresh worker in this browser.
  //
  // It is not a coincidence that the numbers are stable. The Emscripten modules declare their
  // initial memory in the module's own memory section — 16 MiB for qpdf, `-sMAXIMUM_MEMORY=2GB`
  // — so the baseline is a build-time property of the artifact,
  // not an empirical accident. A pinned engine bump that changed it would land here.
  const floor = Number(await page.evaluate(() => window.burrowHarness.minConvergingMemoryBytes()));
  expect(floor, "the worker must report the floor Rust defines").toBeGreaterThan(0);

  const baselineQpdf = Math.min(...rows.map((r) => r.qpdfMiB));
  console.log(
    `baseline [${testInfo.project.name}]: qpdf ${baselineQpdf} MiB, ` +
      `recycling floor ${floor / MIB} MiB`,
  );

  // The relationship the constant encodes: the threshold at the floor is half of it, and the
  // worse engine's baseline must sit below that. If this fails, the floor is too low for the
  // engines as they are now, and the constant needs raising rather than this bound relaxing.
  expect(
    baselineQpdf * MIB,
    `a worker's baseline must sit below the recycling threshold at MIN_CONVERGING_MEMORY_BYTES ` +
      `(${floor / MIB} MiB), or a caller who honours that floor still recycles every operation`,
  ).toBeLessThan(floor / 2);

  // And not absurdly below either: a baseline far under the assumption would mean the floor is
  // needlessly conservative and a mobile page is being told to allow more than it needs.
  expect(
    baselineQpdf * MIB,
    "the baseline has dropped far below what the floor assumes; MIN_CONVERGING_MEMORY_BYTES " +
      "is now more conservative than it needs to be and should be revisited",
  ).toBeGreaterThan(floor / 8);

  // A heap that never grew would mean the reading is not wired up — the same failure the
  // native side hit in 4a-i, where `heap_bytes` was declared and called from nowhere.
  expect(peakQpdf, "an engine heap that never grows means the reading is dead").toBeGreaterThan(0);

  // THE TWO POPULATIONS ARE MEASURED SEPARATELY, because the whole design rests on them being
  // far apart. Measured in Chromium, M1 PR 4a-ii:
  //
  //   ordinary corpus work, every fixture and both engines   pdfium 17.9 MiB   qpdf 16 MiB
  //   xref-bomb.pdf via page_count, ceiling raised to 4 GiB  pdfium 1900.7 MiB
  //   xref-bomb.pdf via structure_check, same ceiling                          qpdf 513 MiB
  //
  // The PDFium column is kept as the measurement it was: it is why the floor is where it is,
  // and deleting it would leave the constant resting on a table that no longer shows the
  // reading that set it. What has changed is that the web no longer loads that engine, so
  // only the qpdf column is measurable here now — and the qpdf figure alone still separates
  // the two populations by two orders of magnitude.
  //
  // Two orders of magnitude between them, with nothing in between. That is what makes the
  // 512 MiB floor in `recycle.rs` a threshold rather than a guess: any value from ~64 MiB to
  // ~512 MiB separates the populations identically.
  const ordinary = rows.filter((row) => row.file !== "xref-bomb.pdf");
  const ordinaryPeak = Math.max(...ordinary.map((r) => r.qpdfMiB));
  expect(
    ordinaryPeak,
    "ordinary corpus work must sit far below the recycling floor, or recycling would be " +
      "discarding healthy workers on every operation",
  ).toBeLessThan(64);

  // And the floor must be REACHABLE by a real file, or recycling is a mechanism that never
  // fires. The bomb is the file that reaches it — and only when a caller has raised
  // `max_memory_bytes` above what the pre-scan would otherwise refuse.
  const bomb = Math.max(...rows.filter((r) => r.file === "xref-bomb.pdf").map((r) => r.qpdfMiB));
  expect(
    bomb,
    "the recycling threshold must be reachable, or it is protection that never runs",
  ).toBeGreaterThan(512);
});
