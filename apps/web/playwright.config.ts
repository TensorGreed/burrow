// Playwright configuration for the engine end-to-end tests.
//
// These tests exist for the half of the web path `cargo test` cannot reach. The Rust
// orchestration — limit ordering, the deadline, error mapping, engine-heap ownership — is
// covered on every target by `core/burrow-engines/src/web/tests.rs` against a fake bridge.
// What needs a real browser is everything below that: the CSP, the integrity-pinned fetch,
// `instantiateWasm`, the classic worker, and whether the engines load at all.
//
// The server is a small static server of our own over the built `dist/`, not `astro dev` and
// no longer `astro preview`. The CSP is part of what is being tested and it is emitted into
// the built HTML, so testing the dev server would be testing something we do not ship.
// `BURROW_HARNESS=1` is what puts the harness route into that build.
//
// It is ours because ADR 0014 §3's second guarantee — "no requests at all after engine init,
// enforced by test" — needs a complete record of what the browser actually requested, and
// `page.on("request")` is not one: browser-reported network events for dedicated workers are
// not equally complete across engines, and the worker is the only place file bytes exist. A
// server's own accept log has no such gap. See `e2e/server.mjs`.

import { defineConfig, devices } from "@playwright/test";

// Must match the origin the CSP was generated against: `connect-src` names absolute URLs,
// because a CSP source expression cannot be origin-relative ('self' takes no path). Serving
// the same build from a different origin would block the engine fetch, which is the policy
// working as intended.
const PORT = Number(process.env.BURROW_TEST_PORT ?? 4321);
const ORIGIN = `http://localhost:${PORT}`;

/**
 * A second local origin that serves nothing and logs everything.
 *
 * Nothing should ever reach it: `default-src 'none'` with no cross-origin source anywhere
 * means the browser refuses such a request before it is sent. `e2e/zero-requests.spec.ts`
 * asserts its log is empty, so the assertion fails if the policy is ever absent,
 * misgenerated, or not inherited by the worker.
 */
const FOREIGN_PORT = Number(process.env.BURROW_FOREIGN_PORT ?? 4322);

export default defineConfig({
  testDir: "./e2e",
  // The engines are 6.5 MB and compile on first load; a cold run is slower than a UI test.
  timeout: 60_000,
  expect: { timeout: 15_000 },
  // No retries. A flaky engine test is a finding, not something to paper over — and a retry
  // would hide exactly the intermittent init race ADR 0006 requirement 1 exists to prevent.
  retries: 0,
  fullyParallel: false,
  // ONE worker, because the request log is shared state.
  //
  // Two browsers processing a corpus at once would interleave their entries, and "no request
  // arrived after this marker" would then be a claim about whichever run happened to be
  // quiet. Each entry does record its user agent, but an assertion that depends on parsing
  // one is weaker than an assertion that does not need to.
  workers: 1,
  reporter: process.env.CI ? [["github"], ["list"]] : [["list"]],

  globalSetup: "./e2e/global-setup.mjs",

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
});

export { FOREIGN_PORT, ORIGIN, PORT };
