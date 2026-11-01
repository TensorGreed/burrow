// The size-budget file's PURE probes (drift and loose-bound rules on planted inputs), without the
// two Astro builds `vitest.config.ts`'s globalSetup makes. `tools/test-size-loose-bound.sh` runs
// the loose rule's mutants against this, so each one costs seconds and not a build.
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    exclude: ["e2e/**", "node_modules/**", "dist/**", "dist-*/**"],
    testTimeout: 60_000,
  },
});
