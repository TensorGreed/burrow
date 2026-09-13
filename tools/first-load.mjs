// Measure the first-load payload of a built site: what a user actually downloads before
// their first file operation can run.
//
// WHY A TOTAL, AND NOT ONLY PER-FILE SIZES
//
// ROADMAP item 11 asks for "the module size" budgeted. Budgeting modules individually is not
// enough, because the thing a user pays is the sum. Three files each growing 4% is a 4%
// regression that no 10%-per-file budget notices, and "which file" is the wrong question to
// gate on -- the payload is the product decision. So the total is the binding budget and the
// per-artifact ones exist to say *where* it went, not to be the gate.
//
// WHAT COUNTS AS FIRST LOAD
//
// Everything fetched before an operation can run:
//
//   * the page's own HTML, and every same-origin asset it references -- CSS, the island
//     runtime once a tool page has one, self-hosted fonts, images. Derived from the markup,
//     so it follows along as pages grow. **One honest limit: assets referenced from inside
//     CSS are not followed** -- a `url(...)` in an `@font-face` or a background. No HTML scan
//     can see those, and a CSS parser here would be the wrong amount of machinery; the total
//     budget is what catches them, since the font still has to be downloaded;
//   * the worker bundle, which carries the Emscripten glue for both engines, the
//     wasm-bindgen glue and the worker itself;
//   * all three .wasm modules -- pdfium, qpdf, and the Rust binding;
//   * the CSP control file, fetched at init to prove an allowlisted request succeeds.
//
// NOT counted: `_headers` (host configuration, never a request), and pages other than the
// entry page. The credits page is large -- it carries eighteen full licence texts -- and is
// deliberately out of scope: nobody loads it on the way to processing a file, and shrinking
// it would mean shipping less of a licence than we are obliged to.
//
// BROTLI, BECAUSE THAT IS WHAT A HOST SERVES. Raw bytes are recorded too, since they are what
// the browser compiles and what the spike measured, but the budget is on the compressed size
// a user's connection actually carries. `zlib.brotliCompressSync` at maximum quality: no
// dependency, and deterministic, which a budget has to be.

import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { brotliCompressSync, constants } from "node:zlib";

/**
 * Every file under `dir`, as paths relative to it, with `/` separators.
 *
 * @param {string} dir
 * @param {string} [prefix]
 * @returns {string[]}
 */
export function walk(dir, prefix = "") {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry);
    const rel = prefix ? `${prefix}/${entry}` : entry;
    return statSync(full).isDirectory() ? walk(full, rel) : [rel];
  });
}

function brotli(bytes) {
  return brotliCompressSync(bytes, {
    params: { [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY },
  }).length;
}

/**
 * The first-load payload of the build in `dir`.
 *
 * Returns `{ entries, total }`, where each entry is `{ path, raw, brotli }` and `total` is
 * `{ raw, brotli }`. Entries are sorted by path so a report diffs cleanly.
 *
 * Throws if the entry page is missing or references an asset that is not in the build --
 * a budget computed over a payload that does not load is worse than no budget.
 *
 * @param {string} dir
 * @param {string} [entryPage]
 * @returns {{ entries: { path: string, raw: number, brotli: number }[],
 *             total: { raw: number, brotli: number } }}
 */
export function firstLoad(dir, entryPage = "index.html") {
  const files = walk(dir);
  if (!files.includes(entryPage)) {
    throw new Error(`${dir}: no ${entryPage}; nothing to measure`);
  }

  const html = readFileSync(join(dir, entryPage), "utf8");

  // Same-origin assets the entry page pulls in. Root-relative only: a cross-origin one would
  // be a privacy violation long before it was a size problem, and `_headers` forbids it.
  //
  // The extension list is deliberately wide. It started as css/js/mjs/wasm, which was true of
  // the build at the time and would have gone quietly wrong the moment a self-hosted font or
  // a logo was added to the layout -- exactly when a size budget needs to notice. Fonts are
  // the live case: `apps/web/CLAUDE.md` permits no third-party fonts, so any font burrow ever
  // uses is a first-load cost.
  const ASSET = /\.(css|js|mjs|wasm|woff2?|ttf|otf|svg|png|jpe?g|webp|avif|gif|ico)$/;
  const referenced = new Set();
  for (const [, url] of html.matchAll(/(?:href|src)="(\/[^"]*)"/g)) {
    const path = url.replace(/[?#].*$/, "").replace(/^\//, "");
    if (!ASSET.test(path)) continue;
    if (!files.includes(path)) {
      throw new Error(`${entryPage} references ${url}, which is not in the build`);
    }
    referenced.add(path);
  }

  // The engine payload. Taken as "everything under engines/" rather than as a list of
  // expected names: a new artifact staged into that directory is part of what a user pays
  // whether or not anyone remembered to add it here.
  const engines = files.filter((f) => f.startsWith("engines/"));
  if (engines.length === 0) {
    throw new Error(`${dir}: no engines/ artifacts; the payload cannot be right`);
  }

  const paths = [entryPage, ...referenced, ...engines].sort();
  const entries = paths.map((path) => {
    const bytes = readFileSync(join(dir, path));
    return { path, raw: bytes.length, brotli: brotli(bytes) };
  });

  return {
    entries,
    total: {
      raw: entries.reduce((n, e) => n + e.raw, 0),
      brotli: entries.reduce((n, e) => n + e.brotli, 0),
    },
  };
}

/**
 * The budget line a file counts against.
 *
 * Keys have to survive a rebuild, and filenames do not. Staged engine artifacts carry a
 * 16-hex content hash, and Astro's own assets carry a Vite hash whose *name* is not stable
 * either -- the shared stylesheet is named after whichever page Vite happened to key the
 * chunk on, so adding a route can rename the home page's CSS without changing a byte of it.
 *
 * So engine artifacts are keyed on their unhashed name, which is stable and meaningful, and
 * everything else -- the entry page and the assets it pulls in -- is pooled into one `page`
 * line. Pooling is right for the page anyway: its files are small, they split and merge at
 * Vite's discretion, and what matters is the shell's total cost, not which chunk holds it.
 *
 * @param {string} path
 * @returns {string}
 */
export function budgetKey(path) {
  if (!path.startsWith("engines/")) return "page";
  return path.replace(/\.[0-9a-f]{16}(\.[A-Za-z0-9]+)$/, "$1");
}

/**
 * Sum a `firstLoad()` result into one entry per budget key.
 *
 * @param {{ entries: { path: string, raw: number, brotli: number }[] }} measurement
 * @returns {Record<string, { raw: number, brotli: number, files: string[] }>}
 */
/**
 * The sha256 of each budget group's bytes, over its files in a stable order.
 *
 * Paired with the sizes in `size-budget.json` so a recording can be held to the build it
 * claims to describe: identical bytes with different recorded numbers is a false record,
 * not drift. See `apps/web/src/size-budget-drift.ts` for the rule and the 236 bytes that
 * went unnoticed without it.
 *
 * Sorted by path rather than taken in `entries` order, because the digest has to be a
 * function of the payload and not of how the walk happened to enumerate it.
 *
 * TWO DIGESTS PER ARTIFACT, and the second exists because of a mistake worth recording.
 *
 * The page's CSP names every engine by its content-hashed URL (ADR 0014), so ANY engine
 * rebuild rewrites `index.html` -- measured in CI, where `qpdf.wasm` is built from source
 * and its hash differs. `normalised` replaces those generated hashes with a fixed token, so
 * "did the page's own content change" is answerable without exempting `page` from checking
 * altogether. `page` is where the design system lives; exempting it for a cause belonging
 * to another artifact would be losing the wrong thing.
 *
 * THE FIRST VERSION RECORDED ONLY THE NORMALISED DIGEST, AND THAT WAS WRONG. Sizes are
 * measured over the REAL bytes, so a normalised match let the check assert exact size
 * equality for files that genuinely differ -- and CI duly reported `page: the bytes are
 * IDENTICAL to the recording, but measured_brotli says 21652 and the build is 21650`. The
 * bytes were not identical; two engine hashes compress differently, and the message was
 * the part that was false.
 *
 * So both are recorded and the three cases are distinguished rather than collapsed:
 * identical raw bytes take the exact check, a normalised-only match takes a tight
 * hash-coupling bound, and a normalised difference is a real change.
 *
 * Deliberately narrow: `.<16 lowercase hex>.` between dots, which is exactly the shape
 * `tools/stage-web-engines.mjs` emits. Vite's own asset hashes are a different length and
 * alphabet, so they are untouched and a CSS change is still a digest change.
 *
 * @param {string} dir
 * @param {Record<string, { files: string[] }>} groups
 * @returns {Record<string, { raw: string, normalised: string }>}
 */
export function digestsByBudgetKey(dir, groups) {
  /** @type {Record<string, { raw: string, normalised: string }>} */
  const digests = {};
  for (const [key, group] of Object.entries(groups)) {
    const raw = createHash("sha256");
    const normalised = createHash("sha256");
    for (const path of [...group.files].sort()) {
      const bytes = readFileSync(join(dir, path));
      raw.update(bytes);
      normalised.update(normaliseEngineHashes(bytes, path));
    }
    digests[key] = { raw: raw.digest("hex"), normalised: normalised.digest("hex") };
  }
  return digests;
}

/** Files whose bytes may quote a generated engine URL. Binaries never do. */
const TEXT_ASSET = /\.(html|css|js|mjs|json|txt|xml|svg)$/;

/**
 * Replace `stage-web-engines.mjs`'s content hashes with a fixed token, in text files only.
 *
 * @param {Buffer} bytes
 * @param {string} path
 * @returns {Buffer}
 */
export function normaliseEngineHashes(bytes, path) {
  if (!TEXT_ASSET.test(path)) return bytes;
  return Buffer.from(bytes.toString("utf8").replace(/\.[0-9a-f]{16}\./g, ".<enginehash>."));
}

export function byBudgetKey(measurement) {
  /** @type {Record<string, { raw: number, brotli: number, files: string[] }>} */
  const groups = {};
  for (const entry of measurement.entries) {
    const key = budgetKey(entry.path);
    groups[key] ??= { raw: 0, brotli: 0, files: [] };
    groups[key].raw += entry.raw;
    groups[key].brotli += entry.brotli;
    groups[key].files.push(entry.path);
  }
  return groups;
}
