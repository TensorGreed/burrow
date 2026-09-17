// The test server's request log, and the marker discipline that makes it assertable.
//
// Extracted from `e2e/zero-requests.spec.ts` when `e2e/merge-pdf.spec.ts` needed the same
// guarantee against the real tool page rather than against the harness. It is shared as a
// MODULE rather than by importing one spec from another: importing a spec file registers its
// tests a second time, so the suite would run twice and the shared log — the very thing being
// asserted on — would interleave.
//
// WHY THE SERVER'S LOG IS THE GROUND TRUTH, and not `page.on("request")`: browser-reported
// network events for dedicated workers are not equally complete across the three engines, and
// the worker is the only place file bytes ever exist. A server's accept log has no such gap.
// See `e2e/server.mjs` and ADR 0014 §3.

import { readFileSync } from "node:fs";
import { appendFile } from "node:fs/promises";

import { expect } from "@playwright/test";

import { LOG_PATH } from "./server.mjs";

export interface LogEntry {
  origin: string;
  method: string;
  url: string;
  at: number;
  agent: string;
  marker?: string;
}

export function readLog(): LogEntry[] {
  return readFileSync(LOG_PATH, "utf8")
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as LogEntry);
}

/** Write a marker straight into the log. No request, so nothing is counted by writing it. */
export async function mark(marker: string): Promise<void> {
  await appendFile(LOG_PATH, `${JSON.stringify({ marker, at: Date.now() })}\n`);
}

/** Everything logged after the last occurrence of `marker`. */
export function since(marker: string): LogEntry[] {
  const entries = readLog();
  const at = entries.map((e) => e.marker).lastIndexOf(marker);
  expect(at, `the marker ${marker} was never written`).toBeGreaterThanOrEqual(0);
  return entries.slice(at + 1).filter((entry) => entry.marker === undefined);
}

/**
 * Requests a **respawn** legitimately makes: the four pinned artifacts, and nothing else.
 *
 * A fresh worker re-fetches the engine modules and the bundle. That is not a leak and must not
 * be asserted away — but it must be recognised by EXACT path, so a request that merely looks
 * engine-ish, or an engine URL carrying a query string, is still caught. The query string is
 * the entire point of `e2e/zero-requests.spec.ts`.
 */
export function isPinnedArtifact(url: string, bundles: "base" | "all" = "base"): boolean {
  return (
    /^\/engines\/(qpdf|burrow_wasm_bg)\.[0-9a-f]{16}\.wasm$/.test(url) ||
    /^\/engines\/burrow-worker\.[0-9a-f]{16}\.js$/.test(url) ||
    /^\/engines\/control\.[0-9a-f]{16}\.txt$/.test(url) ||
    // THE RENDER SET IS ONLY PINNED FOR A PAGE THAT RENDERS, and that default is the whole
    // point of the parameter.
    //
    // `pdfium` left this alternation when spike 0004 took it out of the payload, and ADR 0026
    // brings it back in a bundle a page fetches only when it needs a picture of a page. Adding
    // it unconditionally -- which is what the first version of this change did -- would have
    // made a `/merge-pdf` that fetched 1.9 MB of PDFium **pass every one of the five tool-page
    // "no unexpected request" assertions**, because "expected" would have included it.
    //
    // So the default is the base set, and the five existing call sites get a STRONGER
    // assertion than they had before this change with no edit: they now fail if the page they
    // drive reaches for the render bundle at all. Only a test that is deliberately driving a
    // render passes `"all"`.
    (bundles === "all" &&
      (/^\/engines\/(pdfium|burrow_wasm_render_bg)\.[0-9a-f]{16}\.wasm$/.test(url) ||
        /^\/engines\/burrow-render-worker\.[0-9a-f]{16}\.js$/.test(url)))
  );
}
