// Playwright configuration for the engine end-to-end tests.
//
// These tests exist for the half of the web path `cargo test` cannot reach. The Rust
// orchestration — limit ordering, the deadline, error mapping, engine-heap ownership — is
// covered on every target by `core/burrow-engines/src/web/tests.rs` against a fake bridge.
// What needs a real browser is everything below that: the CSP, the integrity-pinned fetch,
// `instantiateWasm`, the classic worker, and whether the engines load at all.
//
// The server is `astro preview` over the built `dist/`, not `astro dev`. The CSP is part of
// what is being tested and it is emitted into the built HTML, so testing the dev server
// would be testing something we do not ship. `BURROW_HARNESS=1` is what puts the harness
// route into that build.

import { defineConfig, devices } from "@playwright/test";

// Must match the origin the CSP was generated against: `connect-src` names absolute URLs,
// because a CSP source expression cannot be origin-relative ('self' takes no path). Serving
// the same build from a different origin would block the engine fetch, which is the policy
// working as intended.
const PORT = 4321;
const ORIGIN = `http://localhost:${PORT}`;

export default defineConfig({
  testDir: "./e2e",
  // The engines are 6.5 MB and compile on first load; a cold run is slower than a UI test.
  timeout: 60_000,
  expect: { timeout: 15_000 },
  // No retries. A flaky engine test is a finding, not something to paper over — and a retry
  // would hide exactly the intermittent init race ADR 0006 requirement 1 exists to prevent.
  retries: 0,
  fullyParallel: false,
  reporter: process.env.CI ? [["github"], ["list"]] : [["list"]],

  use: {
    baseURL: ORIGIN,
    trace: process.env.CI ? "retain-on-failure" : "off",
  },

  // All three browsers, from 4a-i rather than 4b.
  //
  // The CSP arrangement rests on a rule engines have historically disagreed about — that a
  // blob: worker inherits the creating document's policy while a URL-loaded one does not —
  // and being wrong about it in one browser would silently remove the browser-enforced half
  // of the guarantee *there*, with every Chromium test still green.
  //
  // The other reason is the engines: spike 0001 found browsers differ in which WebAssembly
  // exception-handling encoding they accept, and `pdfium.wasm` is a prebuilt whose encoding
  // is not ours to choose. If an engine will not load in one of these, that is a finding to
  // report, not a test to skip.
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
    { name: "firefox", use: { ...devices["Desktop Firefox"] } },
    { name: "webkit", use: { ...devices["Desktop Safari"] } },
  ],

  webServer: {
    command: "pnpm run build:harness && pnpm run preview --port " + PORT,
    url: ORIGIN,
    reuseExistingServer: !process.env.CI,
    timeout: 300_000,
    stdout: "pipe",
    stderr: "pipe",
  },
});
