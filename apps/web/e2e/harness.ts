// The harness API, typed once.
//
// `apps/web/CLAUDE.md`: "No `any` in committed code." Two specs were reaching into
// `window` — one with `as any` plus an eslint-disable, the other with a long inline
// `as unknown as { ... }` — to do the same job. One declaration is both honest and shorter.
//
// The harness itself is test-only and is removed from production builds; see
// `astro.config.mjs` and `src/production-build.test.ts`.

import type { Page } from "@playwright/test";

/** One operation's outcome, as the worker reports it. */
export interface Reply {
  ok: boolean;
  kind: string;
  /** Computed in Rust, not derived from `kind`. ADR 0009. */
  fatal: boolean;
  message: string;
  pages: number;
  limit: string;
  /** Strings, not numbers: these are `u64` and can exceed 2^53. */
  requested: string;
  allowed: string;
}

/** What a CSP probe inside a worker observed. */
export interface ProbeResult {
  blocked: boolean;
  violations: string[];
  /** Set when the probe worker could not start — a different fact from "blocked". */
  failed?: boolean;
}

export interface BurrowHarness {
  ready(): Promise<boolean>;
  run(
    op: "page_count" | "structure_check",
    bytes: number[],
    options?: { password?: number[]; attemptRecovery?: boolean },
  ): Promise<Reply>;
  spawnCount(): number;
  hasWorker(): boolean;
  fetchFromWorker(url: string): Promise<ProbeResult>;
  workerInheritsCsp(): Promise<boolean>;
}

declare global {
  interface Window {
    burrowHarness: BurrowHarness;
  }
}

/** Open the harness page and wait for both engines to initialise. */
export async function openHarness(page: Page): Promise<void> {
  const { expect } = await import("@playwright/test");
  await page.goto("/harness");
  await expect(page.locator("#status")).toHaveText("engines ready", { timeout: 60_000 });
}
