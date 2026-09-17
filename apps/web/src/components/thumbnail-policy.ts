/**
 * How big a page picture is, and how many of them are held at once.
 *
 * # These numbers were CHOSEN, not measured, and this file is where they are revised
 *
 * Every other number in this app came from something we observed: `LIMITS.maxDurationMs` from
 * `e2e/measure.spec.ts` timing the slowest honest operation, the engine-download figure from
 * `size-budget.json`, the recycle threshold from watching engine heaps. **The two constants
 * here are not of that kind.** They answer a question measurement cannot: not *how much can
 * this machine do*, but *how much should a page ask a stranger's phone for*.
 *
 * They live in one small module rather than inside an island for exactly that reason. A policy
 * spread across two components is a policy nobody can revise with confidence, and these are the
 * numbers most likely to need revising — {@link https://github.com/TensorGreed/burrow/blob/main/docs/adr/0027-what-a-render-promises-and-what-it-refuses.md ADR 0027}'s
 * *"The three revision points"* table names this file and says what evidence would move each.
 *
 * The third revision point is `LIMITS.maxPixels` in `tool-host.ts`, which is a ceiling on the
 * request rather than a description of a thumbnail — so it belongs beside the other limits the
 * page sends, not here.
 *
 * **The measurement that would settle all three is ADR 0015 §7's deferred device test**: render
 * a strip on a real iPhone and watch for tab termination, because on iOS a memory spike kills
 * the tab rather than the worker. Until that has been run, the honest description of these is
 * the one at the top of this comment.
 */

/**
 * A thumbnail's box in CSS pixels: what a tile measures on screen.
 *
 * Portrait, because a page usually is. A page is fitted **inside** this keeping its
 * proportions, so a landscape page comes back shorter and a square one narrower — which is why
 * every rendered page carries its own size rather than the strip assuming this one.
 */
export const THUMBNAIL_CSS_WIDTH = 120;

/** @see THUMBNAIL_CSS_WIDTH */
export const THUMBNAIL_CSS_HEIGHT = 160;

/**
 * The most device pixels per CSS pixel a thumbnail is drawn at. **Revision point 3.**
 *
 * At 2, a tile is at most 240 x 320 = 0.077 Mpx, so **307,200 bytes** of RGBA.
 *
 * **The largest single lever here, and the one to pull first.** Dropping it to 1 *quarters*
 * every thumbnail — 76,800 bytes each, and the whole window from 19.7 MB to 4.9 MB — at the cost
 * of visibly softer tiles on any phone, which is most phones. 2 is a judgement that the
 * sharpness is worth four times the bytes, made without a phone in front of us. If the device
 * test says otherwise, this is the line to change.
 *
 * Capped rather than followed: a DPR-3 screen would otherwise cost 691,200 bytes a tile for a
 * difference nobody has claimed to see.
 */
export const MAX_DEVICE_PIXEL_RATIO = 2;

/**
 * How many thumbnails are held in memory at once. **Revision point 2.**
 *
 * 64 x 307,200 bytes is about **19.7 MB**, which is **26x** below ADR 0015 §5's 512 MiB
 * recycle threshold.
 *
 * # It was 32, and the arithmetic that justified 32 was wrong
 *
 * ADR 0027 proposed 32 on the claim that *"a 390 px phone shows about 8 tiles and a 1280 px
 * desktop about 27, so the window is never what a person notices"*. The test below computed
 * it and it is not: a 1280 x 800 grid of 120 x 160 tiles is 10 columns by 5 rows, which is
 * **50** — more than the window. A window smaller than the viewport blanks tiles while
 * somebody is looking at them, which is the one thing this policy must not do.
 *
 * So the floor is stated as a rule rather than as a number: **the window may not go below what
 * a viewport shows.** 64 clears a 1280 x 800 grid with a row of margin either side.
 *
 * # The residual, on the machine best able to absorb it
 *
 * A 1920 x 1080 grid is 112 tiles, which is more than 64. On such a screen the window
 * *is* the constraint and the tiles furthest from the scroll position will be redrawn when
 * they come back — a visible cost, on a desktop, which is the direction to fail in: the reason
 * this ceiling exists at all is the phone, where about 18 tiles are visible and 64 is three
 * and a half times that. Raising it to cover every desktop would spend the phone's budget to
 * buy a desktop's smoothness.
 *
 * **This is not ADR 0020's rejected "first N pages only."** That was a *feature* window —
 * pictures for part of a document and numbers for the rest, with nothing explaining the
 * boundary. This is a *viewport* window: every page gets a thumbnail when you look at it. The
 * two look alike from a distance and differ in the only way that matters to the person using
 * it.
 */
export const LIVE_THUMBNAIL_WINDOW = 64;

/**
 * The device pixel ratio to draw at: the browser's, capped.
 *
 * Injected rather than read from `globalThis`, so a test can drive every branch without a
 * browser and so the cap is one comparison in one place.
 *
 * A ratio that is absent, zero, negative or not finite falls back to 1 — those are what a
 * headless context, a stubbed `window` and a hostile override look like, and a thumbnail drawn
 * at NaN device pixels is a canvas of nothing.
 */
export function devicePixelRatio(reported: number | undefined): number {
  if (reported === undefined || !Number.isFinite(reported) || reported < 1) return 1;
  return Math.min(reported, MAX_DEVICE_PIXEL_RATIO);
}

/**
 * The box, in **device** pixels, to ask the renderer for.
 *
 * This is the number that reaches `render_begin`, and it is bounded by construction: at the
 * DPR cap it is 240 x 320, which is 1/54th of `LIMITS.maxPixels`. A person cannot reach the
 * ceiling through the strip.
 */
export function thumbnailBox(reported: number | undefined): { width: number; height: number } {
  const ratio = devicePixelRatio(reported);
  return {
    width: Math.round(THUMBNAIL_CSS_WIDTH * ratio),
    height: Math.round(THUMBNAIL_CSS_HEIGHT * ratio),
  };
}

/**
 * How many bytes a full window of thumbnails occupies at `reported` device pixel ratio.
 *
 * Exported because it is the arithmetic the ADR quotes, and a number quoted in prose that
 * nothing computes is a number that goes stale. `src/components/thumbnail-policy.test.ts`
 * holds the ADR's figures to this function.
 */
export function windowBytes(reported: number | undefined): number {
  const box = thumbnailBox(reported);
  return box.width * box.height * 4 * LIVE_THUMBNAIL_WINDOW;
}
