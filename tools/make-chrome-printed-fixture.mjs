#!/usr/bin/env node
// Write `tests/conformance/fixtures/chrome-printed.pdf`: a page of our own content printed to PDF
// by headless Chromium, the producer behind every "Save as PDF" in Chrome (Skia/PDF).
//
// WHY (owner, 2026-10-15). A split regression on Chrome-printed PDFs shipped in v0.1.6 because no
// committed fixture came from that producer, so no census or e2e run ever opened one. This puts one
// in the conformance corpus, which every operation's tests, the open census and the web
// differential all read, so a rule that refuses Chrome's output goes red before a release.
//
// WHAT IS ON THE PAGE, and why each part is there:
//   - paragraphs and a table: Chrome's ordinary text and line drawing;
//   - an image this script generates (a 32x32 PNG gradient, no external file): an image XObject;
//   - links: `href="#"`, which is what makes Chrome write an empty-name destination (`/Dest /`),
//     and an in-page anchor, which writes a named one;
//   - TWO PAGES, the second section on its own, so split has a document to cut and the in-page
//     link points across the cut.
// No external URL, so the PDF names no third-party origin.
//
// THE FONT IS OURS TO SHIP. Chrome embeds a subset of whatever face it renders with, and a system
// font would put a third-party font into a committed PDF (#154). Every glyph here is set in the
// site's own Atkinson Hyperlegible Next (`apps/web/public/fonts/`, SIL OFL 1.1, notice beside it),
// inlined as a data: URL, and this script refuses unless the browser reports that face loaded.
//
// THE CLAIMS ARE ENFORCED ON THE OUTPUT, not only intended: after printing, every `/FontName`
// must be a subset of that face (`ABCDEF+AtkinsonHyperlegibleNext-...`), and neither the bytes nor
// any stream they inflate to may carry a `/URI` or an `http`. Otherwise nothing is written.
//
// A TYPE 3 PATH, NOT CHROME'S USUAL ONE, stated: the only OFL face in the repository is a
// VARIABLE font, and Chrome embeds a variable face as Type 3 glyphs. An ordinary Chrome PDF set in
// a static font takes the Type0/CIDFontType2 path instead, which this fixture does not exercise.
// A static instance would need fontTools, which is not a dependency of this repository; the
// regression this fixture exists for (the empty-name destination) does not depend on the font.
//
// NOT BYTE-REPRODUCIBLE, stated rather than hidden: Chrome writes a creation date and its own
// version into the file. The committed PDF is pinned by sha256 in `expectations.json`; re-running
// this changes the hash, which is a regeneration to commit deliberately, never a drift to accept.
//
// Usage (from the repository root): node tools/make-chrome-printed-fixture.mjs [--tagged]
// It runs Playwright's Chromium from `apps/web/node_modules`, so `pnpm install` there first.

import { createRequire } from "node:module";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync, inflateSync } from "node:zlib";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const webApp = join(repo, "apps", "web");
const require = createRequire(join(webApp, "package.json"));
const { chromium } = require("@playwright/test");

// `--tagged` prints the same page with Chrome's accessibility tags (`tagged: true`), which writes
// the same empty destination as a link annotation's value followed by a line break rather than by
// `>>`, plus an empty `/Dests` key; the default is the untagged shape.
const tagged = process.argv.includes("--tagged");
const out = join(
  repo,
  "tests",
  "conformance",
  "fixtures",
  tagged ? "chrome-printed-tagged.pdf" : "chrome-printed.pdf",
);
// AS A data: URL. A `file://` URL is refused from the `about:blank` page `setContent` makes,
// measured: the face did not load and the check below refused to print.
const font = `data:font/woff2;base64,${readFileSync(
  join(webApp, "public", "fonts", "atkinson-hyperlegible-next-v2.001-latin.woff2"),
).toString("base64")}`;

/** A 32x32 RGB PNG gradient, built here so the fixture depends on no image file. */
function gradientPng() {
  const size = 32;
  const rows = [];
  for (let y = 0; y < size; y += 1) {
    const row = [0]; // filter type: none
    for (let x = 0; x < size; x += 1) row.push(x * 8, y * 8, 128);
    rows.push(...row);
  }
  const crcTable = Array.from({ length: 256 }, (_, n) => {
    let c = n;
    for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    return c >>> 0;
  });
  const crc = (bytes) => {
    let c = 0xffffffff;
    for (const b of bytes) c = crcTable[(c ^ b) & 0xff] ^ (c >>> 8);
    return (c ^ 0xffffffff) >>> 0;
  };
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const sum = Buffer.alloc(4);
    sum.writeUInt32BE(crc(body));
    return Buffer.concat([length, body, sum]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8; // bit depth
  header[9] = 2; // colour type: RGB
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(Buffer.from(rows))),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const image = `data:image/png;base64,${gradientPng().toString("base64")}`;

const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Chrome-printed fixture</title>
<style>
  @font-face { font-family: "Fixture"; src: url("${font}") format("woff2"); }
  * { font-family: "Fixture"; }
  body { margin: 48px; font-size: 14px; }
  table { border-collapse: collapse; margin: 16px 0; }
  td, th { border: 1px solid #333; padding: 4px 8px; }
  #second { break-before: page; }
</style></head>
<body>
  <h1>A page printed by Chromium</h1>
  <p>This document is generated by burrow's own test tooling. Every word in it is ours, and its
  only purpose is to be a file of the kind Chrome writes when a page is saved as a PDF.</p>
  <p>The table, the picture and the links below exercise Chrome's text, line drawing, image and
  link writing. <a href="#">This link points at nothing</a>, and
  <a href="#second">this one points at the second section</a>.</p>
  <table>
    <tr><th>Item</th><th>Count</th><th>Note</th></tr>
    <tr><td>Paragraphs</td><td>3</td><td>plain text</td></tr>
    <tr><td>Images</td><td>1</td><td>a generated gradient</td></tr>
    <tr><td>Links</td><td>2</td><td>one empty, one in-page</td></tr>
  </table>
  <img src="${image}" width="96" height="96" alt="A generated gradient">
  <h2 id="second">Second section</h2>
  <p>The in-page link above lands here.</p>
</body></html>`;

/** Refuse the PDF unless it holds only the shipped face and names no URL, raw or inflated. */
function enforce(pdf) {
  const raw = pdf.toString("latin1");
  const texts = [raw];
  const streams = /stream\r?\n/g;
  let match;
  while ((match = streams.exec(raw)) !== null) {
    const start = match.index + match[0].length;
    const end = raw.indexOf("endstream", start);
    if (end < 0) break;
    try {
      texts.push(inflateSync(pdf.subarray(start, end)).toString("latin1"));
    } catch {
      // Not a flate stream, or one that ends at a different byte: its raw bytes are in `raw`.
    }
  }
  const names = [...raw.matchAll(/\/FontName\s*\/([^\s/<>[\]()]+)/g)].map((m) => m[1]);
  const foreign = names.filter((name) => !/^[A-Z]{6}\+AtkinsonHyperlegibleNext-/.test(name));
  if (names.length === 0 || foreign.length > 0) {
    throw new Error(
      `refusing: the PDF embeds ${names.length} font(s), not all the shipped face: ${foreign.join(", ")}`,
    );
  }
  const urls = texts.filter((text) => text.includes("/URI") || /http/i.test(text)).length;
  if (urls > 0) {
    throw new Error(`refusing: ${urls} part(s) of the PDF carry a /URI or an http reference`);
  }
  console.log(
    `make-chrome-printed-fixture: ${names.length} embedded font(s), all the shipped face; ` +
      `${texts.length} part(s) scanned, raw and inflated, with no /URI and no http`,
  );
}

const browser = await chromium.launch();
try {
  const page = await browser.newPage();
  await page.setContent(html, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
  // REFUSE A FALLBACK FACE: every glyph must be the shipped OFL font, or the PDF carries a
  // system font nobody audited.
  const loaded = await page.evaluate(() =>
    [...document.fonts].filter((f) => f.family.includes("Fixture") && f.status === "loaded").length,
  );
  if (loaded !== 1) {
    throw new Error(`the fixture font did not load (${loaded} face(s) loaded); refusing to print`);
  }
  const pdf = await page.pdf({ format: "A4", printBackground: true, tagged });
  enforce(Buffer.from(pdf));
  writeFileSync(out, pdf);
  console.log(`make-chrome-printed-fixture: wrote ${out} (${pdf.length} bytes) with ${browser.version()}`);
} finally {
  await browser.close();
}
