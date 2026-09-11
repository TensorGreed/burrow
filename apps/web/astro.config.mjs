// @ts-check
import { rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { defineConfig } from "astro/config";
import svelte from "@astrojs/svelte";

/**
 * Keep the engine harness out of production builds.
 *
 * `src/harness/harness.astro` drives a real file through the real worker and exposes
 * `window.burrowHarness` -- exactly what a test needs and exactly what a shipped site must
 * not carry. It lives outside `src/pages/`, so it is not a route at all unless this
 * integration injects one, which it does only under `BURROW_HARNESS=1`.
 *
 * Injecting rather than deleting afterwards matters: a build that produced the page and
 * then removed it still shipped a shared CSS chunk Vite had named `harness.<hash>.css`
 * after it. Not a code leak, but production output named after a test fixture.
 *
 * Two scripts cannot work this way -- the harness driver and the CSP probe worker must be
 * classic scripts at stable URLs, so they live in `public/` and are copied verbatim like
 * any other asset. Those are removed from `dist/` instead.
 *
 * `src/production-build.test.ts` asserts both halves, and includes a control that builds
 * WITH the flag, so an integration that excluded unconditionally would fail rather than
 * look perfect.
 *
 * @returns {import("astro").AstroIntegration}
 */
function harnessGating() {
  const included = process.env.BURROW_HARNESS === "1";
  return {
    name: "burrow:harness-gating",
    hooks: {
      "astro:config:setup": ({ injectRoute, logger }) => {
        if (!included) {
          return;
        }
        logger.warn("BURROW_HARNESS=1 -- /harness IS in this build. Do not deploy it.");
        injectRoute({
          pattern: "/harness",
          entrypoint: "./src/harness/harness.astro",
        });
      },
      "astro:build:done": async ({ dir }) => {
        if (included) {
          return;
        }
        for (const testOnly of ["./burrow-harness.js", "./burrow-csp-probe.js"]) {
          await rm(fileURLToPath(new URL(testOnly, dir)), { force: true });
        }
      },
    },
  };
}

// See docs/adr/0005-web-stack.md.
//
// Static output, one indexable page per tool, interactivity opt-in per island. There is
// deliberately no adapter and no server runtime: no server exists that could receive a
// user's file.
export default defineConfig({
  output: "static",
  integrations: [svelte(), harnessGating()],
  vite: {
    build: {
      // The wasm module dominates the payload, so keep JS chunking predictable, and
      // inline nothing implicitly: we want to see what actually ships.
      target: "es2022",
      assetsInlineLimit: 0,
    },
    worker: {
      format: "es",
    },
  },
});
