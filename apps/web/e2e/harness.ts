// The harness API, as Playwright sees it.
//
// The shape itself lives in `src/host/harness-api.d.ts`, beside the file that implements it,
// and is re-exported here. It was declared only on this side until M1 PR 4a-ii, which meant
// the implementation was never checked against the shape its callers assumed — a rename on one
// side type-checked cleanly on the other.
//
// `apps/web/CLAUDE.md`: "No `any` in committed code." Two specs were reaching into `window` —
// one with `as any` plus an eslint-disable, the other with a long inline `as unknown as { ... }`
// — to do the same job. One declaration is both honest and shorter.
//
// The harness itself is test-only and is removed from production builds; see
// `astro.config.mjs` and `src/production-build.test.ts`.

import type { Page } from "@playwright/test";

export type {
  BurrowHarness,
  HarnessArming,
  HarnessLimits,
  ProbeResult,
  Reply,
} from "../src/host/harness-api.js";

// The `export type` above is what pulls in the declaration, and with it the
// `Window.burrowHarness` augmentation `page.evaluate` callbacks rely on. A side-effect
// `import` of the same path would be a RUNTIME import of a file that does not exist at
// runtime — it is a `.d.ts` — and Playwright fails to collect the suite at all.

/** Open the harness page and wait for both engines to initialise. */
export async function openHarness(page: Page): Promise<void> {
  const { expect } = await import("@playwright/test");
  await page.goto("/harness");
  await expect(page.locator("#status")).toHaveText("engines ready", { timeout: 60_000 });
}
