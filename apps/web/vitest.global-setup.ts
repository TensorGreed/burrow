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
  //
  // NODE_ENV=production, EXPLICITLY, because vitest sets `NODE_ENV=test` in this process and
  // it is inherited by the builds below. That is not cosmetic: Svelte publishes its client
  // runtime under an export condition, so a build that is not `production` resolves the
  // DEVELOPMENT runtime -- 41,744 bytes against 31,063, plus a differently-named chunk. Every
  // build-output test would then be asserting against something no deploy produces, and the
  // size budget would be recording a payload 10 KB larger than the real one.
  //
  // It was invisible until the first island shipped, because a build with no Svelte component
  // in it pulls in no Svelte runtime to be wrong about. Measured in M1 PR B3, by the budget's
  // own drift probe refusing to match a build made minutes earlier by `pnpm build`.
  const clean: NodeJS.ProcessEnv = { ...process.env, NODE_ENV: "production" };
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
