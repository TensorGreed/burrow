// #199's wasm-heap half, by test: after a redaction, the text it removed is in neither wasm heap
// the worker could hand to the next document -- burrow's, because every Rust block is wiped when
// it is freed, and qpdf's, because the worker is recycled.
//
// THE DISCHARGE THIS IS (docs/ROADMAP.md, #199's row): "a canary drawn in an uncompressed
// content stream is absent, after a redaction and before the worker is reused, from both wasm
// heaps". qpdf's object cache holds the decoded page for the life of the document and the C API
// cannot reach it, so "absent before reuse" is met there by there being no reuse: the reply asks
// for recycling and the host discards the worker. Burrow's heap is held directly.
//
// HOW IT LOOKS. The heap canary is a harness prologue (`heapScanPrologue` in
// `src/host/harness-driver.js`): it records every WebAssembly memory the worker instantiates and,
// as the worker posts its reply -- after the Rust call returned and the worker's `finally` wiped
// its own copies -- counts the canary in each. Nothing of it is in a production build.
//
// THE WITNESS, and what was measured. A scan that finds nothing anywhere could be a scan that
// cannot see, so the prologue also samples both heaps on entry to the first bridge calls of the
// operation and keeps the peak: the canary must be seen in each heap WHILE the redaction runs, and
// gone from both when it replies. Measured while writing this (Chromium): peak 1 in qpdf's (the
// input) and 2 in burrow's (the decoded page), 0 and 0 at the reply.
//
// WHAT IT CAN FAIL ON, measured while writing this, each mutation asserted applied and rebuilt:
// - the reply not asking for recycling: red;
// - `Session::drop` freeing the input without wiping it: red, qpdf's heap holds the canary. Only
//   because the page is padded and the canary is at its end -- unpadded, qpdf's later allocations
//   reused the freed block and hid the missing wipe;
// - **burrow's allocator swapped back to `System`: GREEN.** The Rust copies held the canary while
//   the operation ran (peak 2) and none at the reply, wipe or no wipe: the redaction's own later
//   allocations -- the output document, about the size of the decoded page -- reuse those blocks
//   first. So this test does NOT hold `WipeOnFree` on the web. `core/burrow-engines/tests/
//   wipe_on_free.rs` holds the allocator natively, against a control that finds the canary in
//   freed blocks; that the web modules declare it was verified by disassembly (#283's review).
//
// qpdf's heap is clean at the reply too, for this document: an unfiltered stream's decoded bytes
// are a slice of the input, which `Session::drop` wipes, and the copies the bridge hands out are
// wiped as it hands them. That is not a claim about qpdf's object cache in general, which the C
// API cannot reach -- the recycle is what covers it, and the test asserts the recycle rather than
// inferring it is unneeded. A Flate-compressed page left 0 in both heaps at the reply too, but
// its peaks were 0 as well, so it vouched for nothing and is not here.

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";

/** Distinctive, and inside the font's /FirstChar 32 /LastChar 126. */
const CANARY = "BURROWHEAPCANARY5197";

/** The page's upper band, in display units, over the canary at (72, 700) on a 792-point page. */
const REGION = { left: 40, top: 40, width: 500, height: 120 };

/** A one-page document: the canary inside the region, uncompressed, and a kept word outside it. */
function canaryDocument(): Uint8Array {
  const widths = Array(95).fill("600").join(" ");
  // PADDED, AND THE CANARY LAST. A small input's freed block is reused by qpdf's later
  // allocations, which overwrote it whether or not it had been wiped: unpadded, freeing the input
  // unwiped (`Session::drop`) still read 0 here. Past 64 KiB, with the canary at the block's far
  // end, reuse of its start no longer hides a missing wipe.
  const padding = "q Q ".repeat(16 * 1024);
  const content = `${padding}\nBT /F1 24 Tf 72 300 Td (KEPT) Tj ET\nBT /F1 24 Tf 72 700 Td (${CANARY}) Tj ET\n`;
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] " +
      "/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
    `<< /Length ${content.length} >>\nstream\n${content}\nendstream`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 126 " +
      `/Widths [${widths}] /Encoding /WinAnsiEncoding >>`,
  ];
  let out = "%PDF-1.7\n";
  const offsets: number[] = [];
  objects.forEach((body, i) => {
    offsets.push(out.length);
    out += `${i + 1} 0 obj\n${body}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) out += `${String(offset).padStart(10, "0")} 00000 n \n`;
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Uint8Array.from(out, (c) => c.charCodeAt(0));
}

test("after a redaction, burrow's heap holds none of the removed text, and the worker is recycled", async ({
  page,
}) => {
  const bytes = canaryDocument();
  expect(new TextDecoder().decode(bytes)).toContain(`(${CANARY}) Tj`);

  await openHarness(page);
  await page.evaluate(
    (canary) => window.burrowHarness.armRedaction({ heapCanary: canary }),
    CANARY,
  );
  const reply = await page.evaluate(
    ({ bytes, region }) =>
      window.burrowHarness.redactDocument("canary.pdf", new Uint8Array(bytes), 1, [1], region),
    { bytes: Array.from(bytes), region: REGION },
  );
  expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(true);

  const scans = await page.evaluate(() => window.burrowHarness.redactHeapScans());
  // ONE REPLY, ONE SCAN: a scan per reply the worker posted, and the redaction posts one.
  expect(scans, "the heap canary saw no reply").toHaveLength(1);
  const [scan] = scans;
  // TWO MEMORIES, BY NAME: the qpdf module's and burrow's. A capture that missed one would
  // otherwise report a clean scan of half the heaps.
  expect(scan.map((entry) => entry.heap).sort(), JSON.stringify(scan)).toEqual(["burrow", "qpdf"]);
  const burrow = scan.find((entry) => entry.heap === "burrow")!;
  const qpdf = scan.find((entry) => entry.heap === "qpdf")!;

  // THE WITNESSES FIRST: while the operation ran, the scan saw the canary in EACH heap -- the input
  // copied into qpdf's, the decoded page Rust held in burrow's. A heap whose peak is zero is a heap
  // this scan cannot vouch for, and its final zero would mean nothing.
  expect(qpdf.peak, "the scan never saw the canary in qpdf's heap").toBeGreaterThan(0);
  expect(burrow.peak, "the scan never saw the canary in burrow's heap").toBeGreaterThan(0);
  // THE CLAIM: at the reply -- the Rust call returned, every copy freed -- neither heap holds it.
  expect(burrow.hits, `burrow's heap (${burrow.bytes} bytes) still holds the removed text`).toBe(0);
  expect(qpdf.hits, `qpdf's heap (${qpdf.bytes} bytes) still holds the removed text`).toBe(0);

  // AND QPDF'S HEAP IS NOT REUSED: the reply asked for recycling, and the host discarded the worker.
  expect(reply.recycle, "a redaction must ask for its worker to be recycled").toBe(true);
});
