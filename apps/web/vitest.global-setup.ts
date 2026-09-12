// Produce the two builds every build-output test reads, once per `vitest run`.
//
// WHY THIS EXISTS
//
// Three test files now assert against what a build actually contains -- what does not ship
// (`production-build.test.ts`), what the licences require to ship (`credits.test.ts`), and
// what it costs to ship (`size-budget.test.ts`). Each Astro build takes seconds, and vitest
// runs test files in separate workers, so a `beforeAll` in each file means one build per
// file and no way to share. Building here means two builds total, which is fewer than the
// two `production-build.test.ts` was already paying on its own.
//
// TWO BUILDS, NOT ONE, AND THE SECOND IS THE CONTROL
//
// `dist-production-check/` is built with **no** `BURROW_HARNESS` in the environment: it is
// what a deploy runs, and the only version a deploy depends on. `dist-harness-check/` is
// built with the flag set, and exists so "the harness is absent" is distinguishable from
// "the harness was never built". Without it, an integration that deleted the route
// unconditionally would pass every exclusion assertion.
//
// Both go through `pnpm run prebuild` first, which is the real staging path -- engines
// staged and content-hashed, the CSP generated from them, and the credits data generated
// from `engines/licenses.toml`. A test that skipped it would be checking a build no deploy
// produces.

import { execFileSync } from "node:child_process";
import { rmSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const webApp = dirname(fileURLToPath(import.meta.url));

/** Where each build lands. Both are gitignored. */
export const PRODUCTION_DIR = resolve(webApp, "dist-production-check");
export const HARNESS_DIR = resolve(webApp, "dist-harness-check");

export default function setup() {
  // No BURROW_HARNESS: prebuild does not read it, but a stray value in the developer's
  // shell must not reach the production build below.
  const clean = { ...process.env };
  delete clean.BURROW_HARNESS;

  execFileSync("pnpm", ["run", "prebuild"], { cwd: webApp, env: clean, stdio: "pipe" });

  for (const [dir, env] of [
    [PRODUCTION_DIR, clean],
    [HARNESS_DIR, { ...clean, BURROW_HARNESS: "1" }],
  ] as const) {
    rmSync(dir, { recursive: true, force: true });
    execFileSync("pnpm", ["exec", "astro", "build", "--outDir", dir], {
      cwd: webApp,
      env,
      stdio: "pipe",
    });
  }
}
