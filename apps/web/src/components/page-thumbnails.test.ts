import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { LIMITS } from "./tool-host";
import {
  LIVE_THUMBNAIL_WINDOW,
  THUMBNAIL_CSS_HEIGHT,
  THUMBNAIL_CSS_WIDTH,
} from "./thumbnail-policy";

const raw = readFileSync(
  fileURLToPath(new URL("./PageThumbnails.svelte", import.meta.url)),
  "utf8",
);

/**
 * The component with its comments blanked out, offsets preserved.
 *
 * The header explains at length why there is no `<img>` here — and a scan of the raw text
 * counts that sentence as an `<img>`. Blanking rather than deleting keeps every offset equal
 * to the real file's, so a reported position still points at the line. The same treatment
 * `multi-output.test.ts` gives `main.js`, for the same reason.
 */
const source = raw
  .replace(/\/\*[\s\S]*?\*\//g, (m) => " ".repeat(m.length))
  .replace(/\/\/[^\n]*/g, (m) => " ".repeat(m.length))
  .replace(/<!--[\s\S]*?-->/g, (m) => " ".repeat(m.length));
const split = readFileSync(fileURLToPath(new URL("./SplitTool.svelte", import.meta.url)), "utf8");
const rotate = readFileSync(fileURLToPath(new URL("./RotateTool.svelte", import.meta.url)), "utf8");

/**
 * The strip, held to the decisions that shaped it.
 *
 * A source scan, and it says so rather than pretending to be behavioural — the parts that need
 * a browser are `e2e/`'s. What these pin are the things that would silently stop being true:
 * a CSS box that drifted from the policy constants, an `<img>` the CSP would refuse in
 * silence, a retry against the page that just killed a worker.
 */
describe("the page-picture strip", () => {
  it("draws into a canvas and never an img, because img-src admits no blob or data URI", () => {
    // ADR 0014, and `apps/web/CLAUDE.md`: the refusal is SILENT, so nothing at run time would
    // tell anybody. An `<img>` here would simply show nothing, on every page, forever.
    expect(source).toContain("<canvas");
    expect(source).not.toMatch(/<img\b/);
    expect(source).not.toContain("createObjectURL");
  });

  it("sizes its frame from the policy constants", () => {
    // The CSS cannot interpolate a constant -- `style-src 'self'` forbids inline styles and a
    // Svelte scoped block is static -- so the two are written twice and this is what stops
    // them drifting. Revision point 2's whole premise is that the policy lives in one place.
    expect(source).toMatch(new RegExp(`width:\\s*${THUMBNAIL_CSS_WIDTH}px`));
    expect(source).toMatch(new RegExp(`height:\\s*${THUMBNAIL_CSS_HEIGHT}px`));
  });

  it("asks for the device-pixel box the policy computes, not the CSS one", () => {
    // A tile is 120x160 CSS pixels and is RENDERED at up to twice that. Asking for the CSS
    // size would produce a soft picture on every phone, which is revision point 3's cost
    // paid by accident rather than on purpose.
    expect(source).toContain("thumbnailBox(");
    expect(source).toContain("boxWidth: box.width");
    expect(source).toContain("boxHeight: box.height");
  });

  it("evicts through the scheduler rather than shifting the oldest off the front", () => {
    // The eviction rule is the terminating condition. Asserting only that the constant is
    // mentioned left the whole eviction loop deletable with both assertions still true.
    expect(source).toMatch(/while \(live\.length >= LIVE_THUMBNAIL_WINDOW\)/);
    expect(source).toContain("evictable(live, wanted, page)");
    expect(source).toContain("releasedState(out, wanted)");
    expect(LIVE_THUMBNAIL_WINDOW).toBeGreaterThan(0);
  });

  it("does not blame a page for a budget the whole request ran out of", () => {
    // A shared deadline is not a page's fault; marking one permanently on that basis showed
    // "Too complex to preview" on a page that is fine.
    expect(source).toMatch(/reply\.limit === "max_duration_ms"/);
  });

  it("marks the page that killed a worker and never asks for it again", () => {
    // `apps/web/CLAUDE.md`: never retry automatically. A retry against the page that just
    // terminated a worker is the one request guaranteed to terminate the next one too, and
    // three of those latch the breaker.
    //
    // THE ASSIGNMENT, NOT THE IDENTIFIER. This pinned `!refused.has(page)` alone, and deleting
    // the line that PUTS anything into `refused` left all eight tests green -- so the strip
    // would have retried the killing page on every pump with nothing to notice. Found by code
    // review, with that deletion run.
    expect(source).toMatch(/refused = new Set\(\[\.\.\.refused, culprit\]\)/);
    expect(source).toMatch(/state: "refused"/);
  });

  it("uses the render bundle, so a page that shows no strip downloads no PDFium", () => {
    // THE CALL, NOT THE IMPORT. `toContain("RENDER")` was satisfied by the import line alone,
    // so swapping the component onto the DOCUMENTS bundle left every test green -- a strip
    // that asked qpdf to render. Found by code review, with that swap run.
    expect(source).toMatch(/createToolHost\([^)]*,\s*RENDER[,)]/);
    expect(source).toContain("host.ensure()");
  });

  it("is mounted by both tools that select pages by number", () => {
    for (const [name, tool] of [
      ["split", split],
      ["rotate", rotate],
    ] as const) {
      expect(tool, `${name} does not import the strip`).toContain(
        'import PageThumbnails from "./PageThumbnails.svelte"',
      );
      expect(tool, `${name} does not mount the strip`).toContain("<PageThumbnails");
    }
  });

  it("sends the same LIMITS every other operation sends", () => {
    // Including `maxPixels`, which is the ceiling ADR 0027 chose. A strip that sent its own
    // limits would be a second place for them to be decided.
    expect(source).toContain("limits: LIMITS");
    expect(LIMITS.maxPixels).toBe(4_194_304);
  });
});
