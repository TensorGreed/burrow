// The start-up bound's number, re-derived from the payload it was derived from.
//
// ADR 0018 sets `DEFAULT_INIT_TIMEOUT_MS` to 240 s by dividing the largest engine module by a
// stated floor bandwidth. Until code review asked, NOTHING re-derived it: every case in
// `worker-host.test.ts` injects its own `initTimeoutMs`, so reverting the constant to 60 s
// broke no test.
//
// The quieter failure is the one that matters. Bump the engine pin to a 10 MB module and the
// doc comment still says its old kbps figure, now meaning something far larger — ABOVE the
// Slow 3G profile the ADR argues it sits below, so the connection the ADR was written about
// would fail start-up again while the number went on looking derived.
//
// SPIKE 0004 MADE THE SLACK LARGE, and this test now says much less than it did. The largest
// module fell from `pdfium.wasm` at 5,315,922 bytes to `qpdf.wasm` at 1,494,731, and 240 s was
// deliberately NOT lowered to match (see `worker-host.js`), so the derived floor went from
// 177 kbps to about 49 — the Slow 3G comparison is satisfied with room for the payload to
// quadruple, and the upper gate that actually binds is the `<= 10 min` near-miss. Raised by
// security review; recorded rather than tightened, because lowering a start-up bound to keep a
// test sharp is trading a real protection for a measurement.
//
// It lives in its own file because `worker-host.test.ts` has an `afterEach` that requires
// every case to have built a host and released it; these cases build nothing.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, test } from "vitest";

import { DEFAULT_INIT_TIMEOUT_MS } from "./worker-host.js";

const webApp = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

describe("the start-up bound's own arithmetic", () => {
  const budget: { artifacts: Record<string, { measured_raw: number }> } = JSON.parse(
    readFileSync(join(webApp, "size-budget.json"), "utf8"),
  );

  /** Chrome's "Slow 3G" profile, which ADR 0018's measurements are taken on. */
  const SLOW_3G_BPS = 400 * 1024;

  test("allows the largest engine module at a bandwidth below Slow 3G", () => {
    const largest = Math.max(
      ...Object.entries(budget.artifacts)
        .filter(([key]) => key.endsWith(".wasm"))
        .map(([, line]) => line.measured_raw),
    );
    expect(largest, "no engine modules in the budget; this derives nothing").toBeGreaterThan(0);

    const floorBps = (largest * 8) / (DEFAULT_INIT_TIMEOUT_MS / 1000);
    // Reported, not just gated: the number is the point, and a reader should not have to
    // rerun the arithmetic to see what the bound currently assumes.
    console.log(
      `  start-up bound: ${DEFAULT_INIT_TIMEOUT_MS / 1000}s for ${largest} bytes ` +
        `= ${Math.round(floorBps / 1024)} kbps floor (Slow 3G is ${SLOW_3G_BPS / 1024} kbps)`,
    );

    expect(
      floorBps,
      `the bound now assumes ${Math.round(floorBps / 1024)} kbps, at or above Chrome's Slow 3G ` +
        `profile — so the connection ADR 0018 was written about would fail start-up again. ` +
        `Either the engines grew or the bound shrank; re-measure and amend the ADR`,
    ).toBeLessThan(SLOW_3G_BPS);
  });

  test("is not so generous that a hung start-up is never noticed", () => {
    // The near-miss. Without it, "below Slow 3G" is satisfied by any enormous number, and the
    // watchdog would stop being one.
    expect(DEFAULT_INIT_TIMEOUT_MS).toBeLessThanOrEqual(10 * 60_000);
  });
});
