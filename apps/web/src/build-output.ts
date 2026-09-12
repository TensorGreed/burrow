// Shared reading of the two builds `vitest.global-setup.ts` produced.
//
// Test-only. Nothing here ships: it lives under `src/` because that is the only place
// vitest's `include` glob looks (see `vitest.config.ts`), and it is imported by
// `*.test.ts` files alone.

import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

/** The repository root. */
export const REPO = resolve(here, "..", "..", "..");

/** A build with **no** `BURROW_HARNESS` set: what a deploy ships. */
export const PRODUCTION_DIR = resolve(here, "..", "dist-production-check");

/** A build with `BURROW_HARNESS=1`: the control for the exclusion assertions. */
export const HARNESS_DIR = resolve(here, "..", "dist-harness-check");

/** Every file under `dir`, as paths relative to it, with `/` separators. */
export function walk(dir: string, prefix = ""): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry);
    const rel = prefix ? `${prefix}/${entry}` : entry;
    return statSync(full).isDirectory() ? walk(full, rel) : [rel];
  });
}

/** Read one file out of a build, as text. */
export function readBuilt(dir: string, relative: string): string {
  return readFileSync(join(dir, relative), "utf8");
}

/**
 * Resolve an href as a static host would, to the file it serves.
 *
 * `/credits` serves `credits/index.html`; `/` serves `index.html`. Used by the reachability
 * assertion, which has to check that a link *lands somewhere* rather than that some page
 * contains a matching string — a link to a route that does not exist would pass the latter.
 * Returns `null` if nothing is served there.
 */
export function resolveHref(files: string[], href: string): string | null {
  if (!href.startsWith("/")) return null;
  const path = href.replace(/[?#].*$/, "").replace(/^\/+|\/+$/g, "");
  for (const candidate of path === ""
    ? ["index.html"]
    : [`${path}/index.html`, `${path}.html`, path]) {
    if (files.includes(candidate)) return candidate;
  }
  return null;
}

/** Every `href="..."` in a page, in document order. */
export function hrefsIn(html: string): string[] {
  return [...html.matchAll(/href="([^"]*)"/g)].map((m) => m[1]);
}
