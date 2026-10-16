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

  it("asks for a CPU canvas every time, which keeps Linux WebKit off the path that crashed (#107)", () => {
    // Every `getContext` in the component, not the first: a second call site added later with a
    // bare `getContext("2d")` would give that canvas a GPU buffer, and its first `putImageData`
    // would reopen #107's race. The contexts are counted so a scan that found none cannot pass.
    const calls = [...source.matchAll(/getContext\(([^)]*)\)/g)].map((m) => m[1]!);
    expect(calls.length, "the component draws with no 2D context at all").toBeGreaterThan(0);
    // EVERY SPELLING COUNTED: `getContext?.(` or `getContext (` would slip past the call pattern
    // above, so the bare token count must equal the calls it matched.
    expect(
      [...source.matchAll(/\bgetContext\b/g)].length,
      "a getContext spelled so the rule above cannot read it",
    ).toBe(calls.length);
    for (const args of calls) {
      expect(args).toMatch(/^"2d",\s*\{\s*willReadFrequently:\s*true\s*\}$/);
    }
    // And the rule is a rule: the bare form it exists to refuse does not match it.
    expect('"2d"').not.toMatch(/^"2d",\s*\{\s*willReadFrequently:\s*true\s*\}$/);
  });

  it("never terminates its worker while a render is in flight, except at the end deadline", () => {
    // Terminating the render worker mid-render crashed Firefox's content process (SIGSEGV in
    // libxul, the whole tab) about one run in ten on the split test that edits the cuts while
    // it runs; releasing only once the render settles measured 0 in 120. A source scan, and it
    // says so: every `discardWorker()` in the component is one of three --
    // - the pause, guarded by `!running`;
    // - the `finally` that runs once the render has settled;
    // - the end gate's `discard` (#199, one document per worker), whose rule -- at once if nothing
    //   renders, on settle, or at `END_DEADLINE_MS` if the render hangs (owner, 2026-10-15) -- is
    //   `strip-lifecycle.test.ts`'s, with fake timers.
    const discards = [...source.matchAll(/discardWorker\(\)/g)].map((m) => m.index!);
    expect(discards.length, "a discardWorker() the three rules do not account for").toBe(3);
    const guarded = discards.filter((at) =>
      /if \(!running\) host\.$/.test(source.slice(at - 25, at)),
    );
    const settled = discards.filter((at) =>
      /finally \{\s*running = false;\s*if \(paused\) \{\s*host\.$/.test(source.slice(at - 600, at)),
    );
    const gated = discards.filter((at) =>
      /createEndGate\(\{\s*discard: \(\) => host\.$/.test(source.slice(at - 80, at)),
    );
    expect(guarded.length, "the pause terminates the worker without checking for a render").toBe(1);
    expect(settled.length, "nothing releases the engine after an in-flight render settles").toBe(1);
    expect(gated.length, "the end gate does not own the document-change end").toBe(1);
    // AND THE GATE IS TOLD WHETHER A RENDER IS IN FLIGHT, and told when it settles.
    expect(source).toMatch(/ending\.request\(running\)/);
    expect(source).toMatch(/finally \{[\s\S]{0,600}ending\.settled\(\)/);
    // AND NOT TWICE: when the pause's own release has just ended the worker, a waiting end is
    // cancelled rather than carried out on top of it.
    expect(source).toMatch(
      /if \(paused\) \{\s*host\.discardWorker\(\);\s*ending\.cancel\(\);\s*\} else \{\s*ending\.settled\(\);\s*\}/,
    );
    // And the gate's deadline does not outlive the strip.
    expect(source).toMatch(/onDestroy\(\(\) => \{[\s\S]{0,200}ending\.cancel\(\)/);
  });

  it("lets go of a tile's pixels only through releaseTiles, at its four sites (#199, D4)", () => {
    // What releasing does -- zero, then drop -- is `strip-release.test.ts`'s. What this pins is
    // that the component goes through it at every one of the four places a tile lets go, and
    // nowhere zeroes by hand, so a fifth site cannot appear without this count moving.
    const sites = [...source.matchAll(/releaseTiles\(([^)]*)\)/g)].map((m) => m[1]!);
    expect(sites.map((args) => /"(\w+)"/.exec(args)?.[1]).sort()).toEqual([
      "document",
      "evict",
      "pagehide",
      "unmount",
    ]);
    // Each at its own place: the document change (untracked), eviction from the window, the
    // pagehide handler (whose result replaces the tiles), and unmount.
    expect(source).toMatch(/untrack\(\(\) => releaseTiles\(tiles, "document"\)\)/);
    expect(source).toMatch(/releaseTiles\(\[stale\], "evict", releasedState\(out, wanted\)\)/);
    expect(source).toMatch(/tiles = releaseTiles\(tiles, "pagehide"\)/);
    expect(source).toMatch(/onDestroy\(\(\) => \{[\s\S]{0,200}releaseTiles\(tiles, "unmount"\)/);
    expect(source, "pixels zeroed by hand, outside releaseTiles").not.toMatch(/\.fill\(0\)/);
    // PIXELS WITH NOWHERE TO GO, in `accept`: no tile for the page, and a tile redrawn over a
    // picture it still held (#199, L2 and L3).
    expect(source).toMatch(/if \(!tile\) \{\s*zeroPixels\(pixels\);\s*return;\s*\}/);
    expect(source).toMatch(/zeroPixels\(tile\.data\?\.data\);\s*tiles\[page - 1\] = \{/);
  });

  it("asks for one batch per request, and tells the end gate each page arrived (owner, 2026-10-16)", () => {
    // What a batch is and what progress does are `strip-lifecycle.test.ts`'s. What this pins is
    // that the strip uses them: every request is `batchOf` the outstanding pages, and every page
    // that arrives -- current or stale -- restarts a waiting end's deadline before anything else.
    expect(source).toMatch(/const pages = batchOf\(outstanding\(\)\);/);
    expect(
      [...source.matchAll(/(?<!function )\boutstanding\(\)/g)].length,
      "a call to outstanding() that is not batched",
    ).toBe(1);
    expect(source).toMatch(/onPage: \(page, pixels\) => \{\s*ending\.progress\(\);/);
  });

  it("zeroes a previous document's picture on arrival and never shows it (#199, D4)", () => {
    // The stale branch of `onPage`: a render for a document the strip no longer shows is zeroed and
    // dropped before anything can paint it. `discardStale` is `strip-lifecycle.test.ts`'s.
    expect(source).toMatch(
      /if \(mine !== generation\) \{\s*discardStale\(pixels\);\s*return;\s*\}\s*accept\(/,
    );
  });

  it("sends the same LIMITS every other operation sends", () => {
    // Including `maxPixels`, which is the ceiling ADR 0027 chose. A strip that sent its own
    // limits would be a second place for them to be decided.
    expect(source).toContain("limits: LIMITS");
    expect(LIMITS.maxPixels).toBe(4_194_304);
  });
});
