// Each bundle's source list and its TypeScript project are the same list, once.
//
// `tools/stage-web-engines.mjs` names the files each worker bundle is concatenated from;
// `src/worker/tsconfig.json` and `tsconfig.render.json` name the files each bundle's type
// check covers. They are the same list written twice, and nothing held them together — the
// tsconfigs' own comments claimed `src/production-build.test.ts` did, and it does not: that
// test asserts markers inside the built bundles, not which files went into them.
//
// The failure is silent in both directions. A file added to a bundle and not to its tsconfig
// ships **unchecked** — in a directory whose whole reason for being type-checked is that it
// computes heap offsets and pointer values. A file in a tsconfig and not in a bundle is a file
// the checker vouches for and nobody runs.
//
// Raised by code review, which asked the question this file is the answer to.
//
// PARSED, NOT IMPORTED. `stage-web-engines.mjs` stages engines and regenerates the CSP as a
// side effect of being loaded, so importing it from a unit test would restage the app on every
// `pnpm test`. `src/reply-shape.test.ts` reads `lib.rs` the same way and for the same reason:
// the committed source is the witness, and it needs no build.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const repo = join(import.meta.dirname, "..", "..", "..", "..");
const stager = readFileSync(join(repo, "tools", "stage-web-engines.mjs"), "utf8");

/** Each bundle's `app:` sources, in order, as `BUNDLES` lists them. */
function bundleSources(): Record<string, string[]> {
  const bundles: Record<string, string[]> = {};
  // One `{ id: "...", ... order: [ ... ] }` per bundle. Deliberately shallow: a regex over a
  // JavaScript literal is fragile in general, and this one is checked by the assertions below
  // — a parse that stopped matching yields an empty map, and an empty map fails.
  const pattern = /id:\s*"(\w+)",[\s\S]*?order:\s*\[([\s\S]*?)\n {4}\],/g;
  for (const [, id, body] of stager.matchAll(pattern)) {
    bundles[id] = [...body.matchAll(/\bapp:\s*"src\/worker\/([\w.-]+)"/g)].map((m) => m[1]);
  }
  return bundles;
}

/** A tsconfig's `include`, minus the ambient declarations, which are not bundle sources. */
function projectSources(file: string): string[] {
  // `tsc` accepts comments; `JSON.parse` does not, and these files are heavily commented.
  const json = readFileSync(join(import.meta.dirname, file), "utf8").replace(/^\s*\/\/.*$/gm, "");
  const include = JSON.parse(json).include as string[];
  return include.filter((name) => !name.endsWith(".d.ts"));
}

describe("each bundle's sources and its TypeScript project", () => {
  const bundles = bundleSources();

  it("finds both bundles and their sources, so a broken parse cannot pass", () => {
    // THE PROBE. Two empty lists compare equal, and a regex that stopped matching would report
    // a clean run over nothing at all — the shape this repository keeps being caught by.
    expect(Object.keys(bundles).sort()).toEqual(["renderWorker", "worker"]);
    for (const [id, sources] of Object.entries(bundles)) {
      expect(sources.length, `${id} has no app sources, so the parse failed`).toBeGreaterThan(3);
    }
  });

  it.each([
    ["worker", "tsconfig.json"],
    ["renderWorker", "tsconfig.render.json"],
  ])("%s is checked by exactly the files it is built from", (id, config) => {
    // IN ORDER, not as a set. The concatenation order is load-bearing — `render-prelude.js`
    // must precede `pdfium.js` because that glue reads `self.Module` at parse time — and a
    // project listing the same files in a different order would type-check a scope the bundle
    // never has.
    expect(projectSources(config)).toEqual(bundles[id]);
  });
});
