// The PDFium half of the heap canary (ADR 0031 section 7), as far as main can honestly hold it.
//
// WHAT THIS HOLDS TODAY. The flatten paths the canary will guard (render worker paint, wipe,
// terminate) do not exist yet. What does exist is the render worker, PDFium's heap, and the Rust
// render module's, and the question the canary will one day ask of them: is this pattern in that
// memory? A scanner that answers "no" is worth nothing unless it demonstrably answers "yes" when
// the pattern IS there. So this file is the scanner's own proof:
//
// - POSITIVE CONTROL. A page with a text and a pixel pattern is drawn by the real render worker,
//   nothing is wiped, and the full scan must FIND each pattern in the heap it is known to be in.
// - NEAR-MISS TWIN. The same page with the text and colour one unit off: the original patterns
//   must be absent, the twin's own present. A scanner that matched anything would pass the first
//   test and fail this one; one that matched nothing would fail the first.
//
// What waits for the flatten ops (the assertions at the reply, every mutation) is listed as text
// in `~/burrow-census/rounds/heap-canary-pdfium/S12-HOOKS.md`; none of it is skipped or parked
// here. The scanner is `heap-scan.ts`.

import { expect, test } from "@playwright/test";

import { openHarness } from "./harness";
import { installRenderHeapScanner, pixelNeedle, scanRenderHeaps, textNeedle } from "./heap-scan";
import type { MemoryScan, Needle } from "./heap-scan";

/** Two plants, one unit apart in the text's last digit and in the fill's blue channel. */
const PLANTS = {
  planted: { text: "BURROWPDFIUMHEAPCANARY5197", rgb: [51, 102, 153] },
  twin: { text: "BURROWPDFIUMHEAPCANARY5198", rgb: [51, 102, 154] },
} as const;
type Plant = keyof typeof PLANTS;

/** A one-page document: the text, and a filled rectangle of the colour (200 x 60 points). */
function pdf(plant: { text: string; rgb: readonly number[] }): Uint8Array {
  const [r, g, b] = plant.rgb.map((v) => (v / 255).toFixed(6));
  const widths = Array(95).fill("600").join(" ");
  const content =
    `BT /F1 24 Tf 72 700 Td (${plant.text}) Tj ET\n` + `${r} ${g} ${b} rg 72 400 200 60 re f\n`;
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] " +
      "/Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
    `<< /Length ${content.length} >>\nstream\n${content}endstream`,
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
  return new Uint8Array(Buffer.from(out, "latin1"));
}

/** Both patterns of both plants; the pixel one in both channel orders PDFium might hold. */
function needles(): Needle[] {
  const all: Needle[] = [];
  for (const [key, plant] of Object.entries(PLANTS)) {
    const [r, g, b] = plant.rgb;
    all.push(textNeedle(`${key}:text`, plant.text));
    all.push(pixelNeedle(`${key}:pixels-bgra`, [b, g, r, 255], 32));
    all.push(pixelNeedle(`${key}:pixels-rgba`, [r, g, b, 255], 32));
  }
  return all;
}

/** Draw `plant`'s page in the real render worker, then scan every memory it holds. */
async function drawAndScan(
  page: import("@playwright/test").Page,
  plant: Plant,
): Promise<MemoryScan[]> {
  await installRenderHeapScanner(page);
  await openHarness(page);
  const bytes = Array.from(pdf(PLANTS[plant]));
  const reply = await page.evaluate(
    (bytes) =>
      window.burrowHarness.renderStrip("planted.pdf", bytes, [1], {
        boxWidth: 612,
        boxHeight: 792,
      }),
    bytes,
  );
  expect(reply.ok, `${reply.kind}`).toBe(true);
  expect(reply.drawn, "the render worker drew no page").toBe(1);
  return scanRenderHeaps(page, needles());
}

/** Total hits for a needle across all memories, and per heap label. */
function hits(scan: MemoryScan[], name: string): Record<string, number> {
  const by: Record<string, number> = {};
  for (const memory of scan) by[memory.heap] = (by[memory.heap] ?? 0) + (memory.hits[name] ?? 0);
  return by;
}

function describe(scan: MemoryScan[]): string {
  return JSON.stringify(scan.map((m) => ({ heap: m.heap, bytes: m.bytes, hits: m.hits })));
}

test("the scan examined both render memories, by name", async ({ page }) => {
  const scan = await drawAndScan(page, "planted");
  const labels = scan.map((m) => m.heap).sort();
  expect(labels, describe(scan)).toEqual(["burrow-render", "pdfium"]);
  for (const m of scan) expect(m.bytes, `${m.heap} is empty`).toBeGreaterThan(0);
});

test("positive control: with no wipe, the planted text and pixels are found in PDFium's heap", async ({
  page,
}) => {
  const scan = await drawAndScan(page, "planted");
  const text = hits(scan, "planted:text");
  const pixels = hits(scan, "planted:pixels-bgra");
  expect(
    text.pdfium ?? 0,
    `the text was not found in pdfium.wasm's memory: ${describe(scan)}`,
  ).toBeGreaterThan(0);
  expect(
    pixels.pdfium ?? 0,
    `the pixels were not found in pdfium.wasm's memory: ${describe(scan)}`,
  ).toBeGreaterThan(0);
});

test("near-miss twin: a page one unit off shows none of the planted patterns, and its own are found", async ({
  page,
}) => {
  const scan = await drawAndScan(page, "twin");
  // THE TWIN IS REALLY THERE: its own text and pixels, so the scan could see this page.
  expect(hits(scan, "twin:text").pdfium ?? 0, describe(scan)).toBeGreaterThan(0);
  expect(hits(scan, "twin:pixels-bgra").pdfium ?? 0, describe(scan)).toBeGreaterThan(0);
  // AND THE PLANT IS NOT: no memory holds the original's text or either pixel order.
  for (const name of ["planted:text", "planted:pixels-bgra", "planted:pixels-rgba"]) {
    for (const [heap, n] of Object.entries(hits(scan, name))) {
      expect(n, `${name} found ${n}x in ${heap}: ${describe(scan)}`).toBe(0);
    }
  }
});
