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
// NOT counted: `_headers` (host configuration, never a request). The credits page is large --
// it carries eighteen full licence texts -- and is deliberately out of scope: nobody loads it
// on the way to processing a file, and shrinking it would mean shipping less of a licence than
// we are obliged to.
//
// WHICH PAGE IS MEASURED, and it is no longer `index.html`. That changed when the first tool
// page landed. People arrive from a search for "merge pdf" and land on `/merge-pdf`, which
// carries an island bundle the home page does not -- so budgeting the home page would have
// budgeted the lightest route while the heaviest one grew unwatched. `heaviestFirstLoad`
// measures EVERY landing route and budgets the largest, and returns the ones it weighed so the
// choice is visible rather than assumed. A route overtaking the recorded one is a finding, not
// a silent substitution: `size-budget.json` records which route was measured, and the test
// fails when the build disagrees.
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

  // AND WHAT THOSE SCRIPTS IMPORT, transitively.
  //
  // A bundled island does not arrive alone: Astro's chunk `import`s the Svelte runtime from a
  // second file, and the browser must have both before anything runs. Scanning only the markup
  // counted the island (21 KB raw) and missed the runtime beside it (31 KB) -- a third of the
  // page's real payload, invisible to the line that exists to watch it, found while splitting
  // the page budget in M1's consolidation batch.
  //
  // Specifiers are RELATIVE to the importing file and are resolved against its directory,
  // because that is what the browser does. Only same-origin relative and root-relative ones
  // are followed; there is nothing else to follow, since no third-party request is permitted
  // on any page.
  const followed = new Set();
  const queue = [...referenced].filter((path) => /\.m?js$/.test(path));
  while (queue.length > 0) {
    const from = queue.pop();
    if (from === undefined || followed.has(from)) continue;
    followed.add(from);
    const source = readFileSync(join(dir, from), "utf8");
    const base = from.includes("/") ? from.slice(0, from.lastIndexOf("/")) : "";
    // Three shapes, because a bundled chunk uses all three and a single clever pattern was
    // already wrong once: the first version bounded the distance between `import` and the
    // specifier at 64 characters, and a minified island's destructured import list is
    // hundreds. It matched nothing and reported a complete payload.
    const specifiers = [
      ...source.matchAll(/\bfrom\s*["']([^"']+)["']/g),
      ...source.matchAll(/(?:^|[\s;}])import\s*["']([^"']+)["']/g),
      ...source.matchAll(/\bimport\(\s*["']([^"']+)["']\s*\)/g),
    ];
    for (const [, specifier] of specifiers) {
      if (
        !specifier.startsWith("./") &&
        !specifier.startsWith("../") &&
        !specifier.startsWith("/")
      ) {
        continue;
      }
      const resolved = specifier.startsWith("/")
        ? specifier.slice(1)
        : normalisePath(base, specifier);
      if (!files.includes(resolved)) {
        throw new Error(`${from} imports ${specifier}, which is not in the build`);
      }
      if (!referenced.has(resolved)) {
        referenced.add(resolved);
        queue.push(resolved);
      }
    }
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
 * Resolve a relative module specifier against the directory of the file that imported it.
 *
 * Deliberately tiny and deliberately not `node:path`: these are POSIX-ish build paths with
 * `/` separators regardless of platform, and `path.resolve` would anchor them to the process's
 * working directory.
 *
 * @param {string} base
 * @param {string} specifier
 * @returns {string}
 */
function normalisePath(base, specifier) {
  const parts = base.length > 0 ? base.split("/") : [];
  for (const segment of specifier.split("/")) {
    if (segment === "." || segment === "") continue;
    if (segment === "..") parts.pop();
    else parts.push(segment);
  }
  return parts.join("/");
}

/**
 * Pages a person can land on, as paths relative to `dir`.
 *
 * Every route except `credits/`, which is out of scope for the reason in the header.
 *
 * @param {string} dir
 * @returns {string[]}
 */
export function landingPages(dir) {
  return walk(dir)
    .filter((path) => path === "index.html" || path.endsWith("/index.html"))
    .filter((path) => path !== "credits/index.html")
    .sort();
}

/**
 * The heaviest landing route's first-load payload, and every route that was weighed.
 *
 * The budget goes on the worst route a person can arrive at, not on whichever one happens to
 * be the site root. `considered` is returned so a report can print what was compared -- a
 * measurement that silently picked one of several is the shape this project treats as no
 * measurement at all.
 *
 * @param {string} dir
 * @returns {{ page: string,
 *             measurement: { entries: { path: string, raw: number, brotli: number }[],
 *                            total: { raw: number, brotli: number } },
 *             considered: { page: string, brotli: number }[] }}
 */
export function heaviestFirstLoad(dir) {
  const pages = landingPages(dir);
  if (pages.length === 0) {
    throw new Error(`${dir}: no landing pages; nothing to measure`);
  }
  const weighed = pages.map((page) => ({ page, measurement: firstLoad(dir, page) }));
  const heaviest = weighed.reduce((a, b) =>
    b.measurement.total.brotli > a.measurement.total.brotli ? b : a,
  );
  return {
    page: heaviest.page,
    measurement: heaviest.measurement,
    considered: weighed.map((w) => ({ page: w.page, brotli: w.measurement.total.brotli })),
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
 * everything else -- the entry page and the assets it pulls in -- is pooled. Pooling is right
 * for the page anyway: its files are small, they split and merge at Vite's discretion, and
 * what matters is the shell's total cost, not which chunk holds it.
 *
 * TWO POOLS, NOT ONE, AND THE SPLIT IS BY KIND RATHER THAN BY NAME.
 *
 * `page-js` is every bundled script; `page` is the markup, the stylesheets and the fonts.
 * They are separated because they differ in one property that the drift check depends on:
 * **a bundled script is not byte-reproducible across architectures and the rest of the page
 * is.** That is measured rather than assumed -- M1 PR B3 pushed a per-file digest breakdown
 * to CI, and against an aarch64 recording the x86_64 build reported identical digests for
 * the markup, both stylesheets and the font, and a different one for the island chunk alone.
 * The bundler's native binary assigns mangled identifiers differently (`f as Ie` here,
 * `f as xe` there).
 *
 * Pooling them together cost the whole line its exact check: `page` went onto
 * `not_byte_reproducible` and a CSS regression of up to 2% could have hidden behind a
 * difference belonging entirely to the JavaScript. Split, the stylesheets and the markup keep
 * the exact check and only the scripts take the bound.
 *
 * The rule is BY KIND so it classifies what does not exist yet. A new stylesheet joins the
 * exact line automatically; a new script joins the bounded one. Naming the island's chunk
 * would have been a list to keep up to date, and this project has already watched one of
 * those rot.
 *
 * @param {string} path
 * @returns {string}
 */
export function budgetKey(path) {
  if (!path.startsWith("engines/")) return /\.m?js$/.test(path) ? "page-js" : "page";
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
 * `tools/stage-web-engines.mjs` emits.
 *
 * AND, IN MARKUP ONLY, Vite's own `.<8 chars>.` asset hash. This is the second thing the page
 * quotes that is not its own: a script chunk's name is a hash of that chunk's CONTENT, and a
 * bundled script is not byte-reproducible across architectures. Without this the markup
 * inherits the JavaScript's unreproducibility through a filename.
 *
 * It is safe HERE and would not be anywhere else, and the reason is the split in `budgetKey`:
 * the scripts live on their own budget line, with their own digest, so a real island change
 * shows up there. A stylesheet's name is likewise backed by its own bytes in this same group.
 * What is given up is exactly one thing -- a pure rename with identical content -- and what
 * is bought is the `page` line keeping its exact check instead of the whole line taking a 2%
 * bound because of something belonging to the JavaScript.
 *
 * This was tried and reverted once before the split existed, with a commit saying it bought
 * nothing. That was true then and is not now: on its own it removed one coupling of three.
 * Outside markup the hashes are left alone, and there is a near-miss for it.
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
  const text = bytes.toString("utf8").replace(/\.[0-9a-f]{16}\./g, ".<enginehash>.");
  if (!/\.html$/.test(path)) return Buffer.from(text);
  return Buffer.from(
    text.replace(/\.[A-Za-z0-9_-]{8}\.(js|css|woff2?|svg|png)\b/g, ".<vitehash>.$1"),
  );
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
