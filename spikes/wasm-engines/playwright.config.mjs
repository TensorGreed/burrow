import { defineConfig } from "@playwright/test";

// Chromium only, headless. The spike's bar is "runs headless in CI", not cross-browser.
export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  // Engine loads plus a 50 MB open are slow the first time.
  timeout: 300_000,
  expect: { timeout: 120_000 },
  reporter: [["list"]],
  use: {
    baseURL: "http://127.0.0.1:8901",
    browserName: "chromium",
  },
  webServer: {
    // Plain static server: no server-side anything, which is also the point.
    command: "npx --yes http-server option1-js-bridge/www -p 8901 -c-1 --silent",
    url: "http://127.0.0.1:8901/index.html",
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
