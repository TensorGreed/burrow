// #199's wasm-heap half, by test: after a redaction, the text it removed is in neither wasm heap
// the worker could hand to the next document -- burrow's, because every Rust block is wiped when
// it is freed, and qpdf's, because the worker is recycled.
//
// THE DISCHARGE THIS IS (docs/ROADMAP.md, #199's row): "a canary drawn in an uncompressed
// content stream is absent, after a redaction and before the worker is reused, from both wasm
// heaps". qpdf's object cache holds the decoded page for the life of the document and the C API
// cannot reach it, so "absent before reuse" is met there by there being no reuse: the reply asks
// for recycling, and the host discards the worker on that flag (`src/host/worker-host.test.ts`
// holds the discard; this spec holds the flag).
//
// HOW IT LOOKS. The heap canary is a harness prologue (`heapScanPrologue` in
// `src/host/harness-driver.js`): it records every WebAssembly memory the worker instantiates and,
// as the worker posts its reply -- after the Rust call returned and the worker's `finally` wiped
// its own copies -- counts the canary in each. Nothing of it is in a production build.
//
// THE WITNESS. A scan that finds nothing anywhere could be a scan that cannot see, so the prologue
// also samples both heaps on entry to every eighth bridge call of the operation and keeps the
// peak: the canary must be seen in each heap WHILE the redaction runs, and gone from both when it
// replies.
//
// FOUR PAGES, because each holds a different wipe and no one page holds them all -- a freed block
// the redaction's own later allocations reuse hides a missing wipe, measured twice:
// - PADDED, the canary at its end: holds `Session::drop`'s wipe of the input in qpdf's heap.
//   Unpadded, qpdf reused the freed input block and a missing wipe read clean. The padding is kept
//   by the redaction, so the output is as large as the page and reuses burrow's freed blocks --
//   this page cannot hold the allocator.
// - DENSE, the canary shown 2,000 times in one text object and nothing kept, uncompressed and
//   Flate: the output is far smaller than what the redaction freed, so burrow's allocator swapped
//   back to `System` leaves 2,001 copies at the reply (red), and `WipeOnFree` none. #199's review
//   corrected this file's first version, which had concluded from the padded page alone that the
//   allocator could not be held on the web; see `DENSE` for the two near shapes that cannot.
// - REFUSED: a page the walk reads and then refuses, which must ask for recycling too -- a refusal
//   has read the decoded page into qpdf's cache as surely as a success.
//
// WHAT IT CAN FAIL ON, each mutation asserted applied and rebuilt through the stamped wasm-pack:
// the recycle dropped; `Session::drop` freeing the input unwiped; `WipeOnFree` swapped for
// `System`; a Rust copy of the text retained (`mem::forget`, the review's): each red.

import { deflateSync } from "node:zlib";

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";

/** Distinctive, and inside the font's /FirstChar 32 /LastChar 126. */
const CANARY = "BURROWHEAPCANARY5197";

/** The page's upper band, in display units, over the canary at y=700 on a 792-point page. */
const REGION = { left: 40, top: 40, width: 500, height: 120 };

/** A one-page document whose content is `content`, its stream optionally Flate-compressed. */
function document(content: string, flate = false): Uint8Array {
  const widths = Array(95).fill("600").join(" ");
  const raw = Buffer.from(content, "latin1");
  const data = flate ? deflateSync(raw) : raw;
  const text = (s: string) => Buffer.from(s, "latin1");
  const bodies: Buffer[][] = [
    [text("<< /Type /Catalog /Pages 2 0 R >>")],
    [text("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")],
    [
      text(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] " +
          "/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
      ),
    ],
    [
      text(`<< /Length ${data.length}${flate ? " /Filter /FlateDecode" : ""} >>\nstream\n`),
      data,
      text("\nendstream"),
    ],
    [
      text(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 126 " +
          `/Widths [${widths}] /Encoding /WinAnsiEncoding >>`,
      ),
    ],
  ];
  const parts: Buffer[] = [text("%PDF-1.7\n")];
  let length = parts[0].length;
  const offsets: number[] = [];
  bodies.forEach((body, i) => {
    offsets.push(length);
    for (const piece of [text(`${i + 1} 0 obj\n`), ...body, text("\nendobj\n")]) {
      parts.push(piece);
      length += piece.length;
    }
  });
  let tail = `xref\n0 ${offsets.length + 1}\n0000000000 65535 f \n`;
  for (const offset of offsets) tail += `${String(offset).padStart(10, "0")} 00000 n \n`;
  tail += `trailer\n<< /Size ${offsets.length + 1} /Root 1 0 R >>\nstartxref\n${length}\n%%EOF\n`;
  parts.push(text(tail));
  return new Uint8Array(Buffer.concat(parts));
}

const KEPT = "BT /F1 24 Tf 72 300 Td (KEPT) Tj ET\n";
const SHOW = `BT /F1 24 Tf 72 700 Td (${CANARY}) Tj ET\n`;
/**
 * The canary shown 2,000 times in ONE text object, and nothing kept: everything the walk decoded
 * is removed, so the output is far smaller than what the redaction freed. Measured under burrow's
 * allocator swapped back to `System` (Chromium): 2,001 copies in burrow's heap at the reply. Two
 * near shapes do NOT hold it, and are why this one is exact: 2,000 separate text objects left 0,
 * and the same object beside a kept line left 2 -- the output, and the edits that build it, reuse
 * the freed blocks.
 */
const DENSE = `BT /F1 24 Tf 72 700 Td ${`(${CANARY}) Tj 0 0 Td `.repeat(2000)}ET\n`;

const PAGES: { name: string; bytes: Uint8Array; ok: boolean }[] = [
  {
    // 64 KiB of kept no-ops, then the canary last: see the header.
    name: "padded, uncompressed",
    bytes: document(`${"q Q ".repeat(16 * 1024)}\n${KEPT}${SHOW}`),
    ok: true,
  },
  { name: "dense, uncompressed", bytes: document(DENSE), ok: true },
  { name: "dense, Flate", bytes: document(DENSE, true), ok: true },
  {
    // A filled rectangle across the region: the walk reads the page, then refuses
    // `[vector-in-region]`.
    name: "refused after reading",
    bytes: document(`${KEPT}${SHOW}0 g 50 680 200 60 re f\n`),
    ok: false,
  },
];

for (const { name, bytes, ok } of PAGES) {
  test(`after a redaction, neither heap holds the removed text, and the worker is recycled: ${name}`, async ({
    page,
  }) => {
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
    expect(reply.ok, `${reply.kind}: ${reply.message}`).toBe(ok);

    const scans = await page.evaluate(() => window.burrowHarness.redactHeapScans());
    // ONE REPLY, ONE SCAN: a scan per reply the worker posted, and the redaction posts one.
    expect(scans, "the heap canary saw no reply").toHaveLength(1);
    const [scan] = scans;
    // TWO MEMORIES, BY NAME: the qpdf module's and burrow's. A capture that missed one would
    // otherwise report a clean scan of half the heaps.
    expect(scan.map((entry) => entry.heap).sort(), JSON.stringify(scan)).toEqual([
      "burrow",
      "qpdf",
    ]);
    const burrow = scan.find((entry) => entry.heap === "burrow")!;
    const qpdf = scan.find((entry) => entry.heap === "qpdf")!;

    // THE WITNESSES FIRST: while the operation ran, the scan saw the canary in EACH heap.
    expect(qpdf.peak, "the scan never saw the canary in qpdf's heap").toBeGreaterThan(0);
    expect(burrow.peak, "the scan never saw the canary in burrow's heap").toBeGreaterThan(0);
    // THE CLAIM: at the reply -- the Rust call returned, every copy freed -- neither heap holds it.
    expect(burrow.hits, `burrow's heap (${burrow.bytes} bytes) still holds the removed text`).toBe(
      0,
    );
    expect(qpdf.hits, `qpdf's heap (${qpdf.bytes} bytes) still holds the removed text`).toBe(0);

    // AND QPDF'S HEAP IS NOT REUSED: the reply asks for recycling, refusal or not.
    expect(reply.recycle, "a redaction must ask for its worker to be recycled").toBe(true);
  });
}
