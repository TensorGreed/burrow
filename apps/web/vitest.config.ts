// Vitest owns the build-output checks; Playwright owns anything needing a browser.
//
// The `exclude` matters: vitest's default `include` glob matches `**/*.spec.ts`, which
// picked up every file in `e2e/` and tried to run Playwright's `test()` under vitest. The
// symptom is four "failed" test files with no useful error, which is a confusing way to
// discover a configuration problem.
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    exclude: ["e2e/**", "node_modules/**", "dist/**", "dist-*/**"],
    // Two real Astro builds, produced ONCE for every test file that reads build output --
    // production-build, credits, and size-budget. vitest runs test files in separate
    // workers, so a per-file `beforeAll` would build once per file and share nothing.
    globalSetup: ["./vitest.global-setup.ts"],
    testTimeout: 300_000,
    hookTimeout: 300_000,
  },
});
