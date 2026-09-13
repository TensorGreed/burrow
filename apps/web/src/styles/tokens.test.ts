// The design tokens are checked, not eyeballed.
//
// Two halves, and both are needed:
//
//   * the rules applied to the real `tokens.css`, gated on the number of comparisons made
//     rather than merely on nothing failing;
//   * each rule applied to a fixture it must accept and a near-miss it must reject, on
//     EVERY run. A contrast check whose parser returned nothing would pass a file with no
//     tokens in it at all, and would look identical to one that checked every pair.
//
// See CLAUDE.md's definition of done: "a rule that matches nothing passes everything; one
// that matches everything fails everything; neither is a check".

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { REPO } from "../build-output.js";
import {
  BOUNDARY_MINIMUM,
  BOUNDARY_TOKENS,
  DECORATIVE_TOKENS,
  TEXT_MINIMUM,
  TEXT_TOKENS,
  checkContrast,
  colourTokens,
  contrast,
  fontFaceUrls,
  luminance,
  parseThemes,
  preloadedHrefs,
} from "./tokens-check.js";

const WEB = join(REPO, "apps", "web");
const read = (relative: string) => readFileSync(join(WEB, relative), "utf8");

const TOKENS_CSS = read("src/styles/tokens.css");
const BASE_CSS = read("src/styles/base.css");
const LAYOUT = read("src/layouts/BaseLayout.astro");

describe("the rules themselves", () => {
  // These run before the rules are pointed at the real file. If the check cannot tell a
  // good value from a bad one, its verdict on `tokens.css` means nothing, and a green run
  // over the real file would be the most misleading possible outcome.

  it("computes the two luminance anchors exactly", () => {
    expect(luminance("#ffffff")).toBeCloseTo(1, 10);
    expect(luminance("#000000")).toBeCloseTo(0, 10);
  });

  it("computes the contrast extremes exactly", () => {
    expect(contrast("#ffffff", "#000000")).toBeCloseTo(21, 6);
    expect(contrast("#767f86", "#767f86")).toBeCloseTo(1, 6);
  });

  it("accepts a pair that clears the text minimum and rejects one just under it", () => {
    // #767f86 on #f2f4f5 is 3.70 — a legitimate boundary, and a deliberate near-miss for
    // text. The gap between the two minimums is the thing most easily got wrong, so it is
    // the pair the probe uses.
    const boundary = contrast("#767f86", "#f2f4f5");
    expect(boundary).toBeGreaterThanOrEqual(BOUNDARY_MINIMUM);
    expect(boundary).toBeLessThan(TEXT_MINIMUM);

    expect(contrast("#5a646e", "#f2f4f5")).toBeGreaterThanOrEqual(TEXT_MINIMUM);
  });

  it("refuses a stylesheet with no :root block rather than reporting it clean", () => {
    expect(() => parseThemes("body { color: red; }")).toThrow(/no `:root/);
  });

  it("refuses a stylesheet with no dark block rather than reporting it clean", () => {
    expect(() => parseThemes(":root { --paper: #ffffff; }")).toThrow(/prefers-color-scheme/);
  });

  it("refuses a :root block that declares nothing", () => {
    expect(() =>
      parseThemes(":root { }\n@media (prefers-color-scheme: dark) { :root { --paper: #000000; } }"),
    ).toThrow(/declares no custom properties/);
  });

  it("does not mistake the dark block's :root for the light one", () => {
    const themes = parseThemes(
      ":root { --paper: #ffffff; --ink: #000000; }\n" +
        "@media (prefers-color-scheme: dark) { :root { --paper: #000000; --ink: #ffffff; } }",
    );
    expect(themes.light.get("paper")).toBe("#ffffff");
    expect(themes.dark.get("paper")).toBe("#000000");
  });

  it("fails a planted low-contrast token, naming it", () => {
    const planted = parseThemes(
      // #b8bec2 on #f2f4f5 is about 1.6:1 — a plausible-looking grey, and unreadable.
      ":root { --paper: #f2f4f5; --ink: #b8bec2; }\n" +
        "@media (prefers-color-scheme: dark) { :root { --paper: #12161a; --ink: #e6ebee; } }",
    );
    const report = checkContrast(planted);
    expect(report.failures.map((f) => `${f.theme}/${f.token}`)).toEqual(["light/ink"]);
    expect(report.checked).toHaveLength(2);
  });

  it("fails a token that exists in one theme and not the other", () => {
    const planted = parseThemes(
      ":root { --paper: #f2f4f5; --ink: #14181c; --signal: #0b6e5e; }\n" +
        "@media (prefers-color-scheme: dark) { :root { --paper: #12161a; --ink: #e6ebee; } }",
    );
    expect(checkContrast(planted).unpaired).toEqual(["signal"]);
  });

  it("flags a colour token that is in no classification list", () => {
    const planted = parseThemes(
      ":root { --paper: #f2f4f5; --mystery: #123456; }\n" +
        "@media (prefers-color-scheme: dark) { :root { --paper: #12161a; --mystery: #abcdef; } }",
    );
    // Neither checked nor knowingly exempt. A new colour must be classified deliberately,
    // because the cost of guessing wrong is an unreadable value that passes.
    expect(checkContrast(planted).unclassified.sort()).toEqual(["dark/mystery", "light/mystery"]);
  });

  it("finds an @font-face url, and does not invent one", () => {
    expect(fontFaceUrls('@font-face { src: url("/fonts/a.woff2") format("woff2"); }')).toEqual([
      "/fonts/a.woff2",
    ]);
    expect(fontFaceUrls('.x { background: url("/img/b.svg"); }')).toEqual([]);
  });

  it("finds a preload href, and does not count a non-preload link", () => {
    expect(preloadedHrefs('<link rel="preload" href="/fonts/a.woff2" as="font" />')).toEqual([
      "/fonts/a.woff2",
    ]);
    expect(preloadedHrefs('<link rel="stylesheet" href="/fonts/a.woff2" />')).toEqual([]);
  });
});

describe("tokens.css", () => {
  const themes = parseThemes(TOKENS_CSS);
  const report = checkContrast(themes);

  it("declares the same colour tokens in both themes", () => {
    expect(report.unpaired).toEqual([]);
  });

  it("classifies every colour token as text, boundary, or decorative-with-a-reason", () => {
    expect(report.unclassified).toEqual([]);
  });

  it("compares every classified token in both themes, and says how many", () => {
    // THE COUNT IS THE MEASUREMENT. Gating on "nothing failed" would pass a run that
    // compared nothing — the exact shape of the engine-licence drift check that reported OK
    // while examining 4 of 15 components. The expected number is derived from the file
    // rather than written down: every colour token that is not the ground and not
    // decorative, in each of the two themes.
    const perTheme = TEXT_TOKENS.length + BOUNDARY_TOKENS.length;
    expect(report.checked).toHaveLength(perTheme * 2);

    const colours = colourTokens(themes.light);
    expect(colours.size).toBe(perTheme + 1 + Object.keys(DECORATIVE_TOKENS).length);
  });

  it("clears the contrast minimum for every token it compared", () => {
    const failures = report.failures.map(
      (f) => `${f.theme}/--${f.token}: ${f.ratio.toFixed(2)}:1, needs ${f.required}:1`,
    );
    expect(failures).toEqual([]);
  });

  it("exempts exactly the tokens it is meant to, and no others", () => {
    // THE EXEMPTION LIST IS PINNED, not merely counted. The assertion above derives its
    // expected number from the same lists `checkContrast` consults, so moving a token from
    // a checked list into `DECORATIVE_TOKENS` keeps both sides balanced. Code review did
    // exactly that -- emptied `BOUNDARY_TOKENS`, exempted `edge` with a plausible reason,
    // and set `--edge` to about 1.1:1 against the ground -- and all eighteen tests passed.
    // Naming the list here makes growing it a deliberate edit to a line that says what
    // changed, which is the difference between an exemption and a hole.
    expect(Object.keys(DECORATIVE_TOKENS).sort()).toEqual(["rule"]);
    expect([...TEXT_TOKENS].sort()).toEqual(["ink", "ink-quiet", "refuse", "signal"]);
    expect([...BOUNDARY_TOKENS].sort()).toEqual(["edge"]);
  });

  it("gives every decorative exemption a reason", () => {
    for (const [token, reason] of Object.entries(DECORATIVE_TOKENS)) {
      expect(
        reason.length,
        `--${token} is exempt from a contrast ratio with no reason given`,
      ).toBeGreaterThan(20);
    }
  });
});

describe("the font", () => {
  it("preloads every face the stylesheets reference", () => {
    const faces = [...fontFaceUrls(TOKENS_CSS), ...fontFaceUrls(BASE_CSS)];
    // A near-miss guard on the check itself: if the stylesheets stopped declaring any face,
    // "every face is preloaded" would be vacuously true.
    expect(
      faces.length,
      "no @font-face found, so this assertion would check nothing",
    ).toBeGreaterThan(0);

    const preloaded = preloadedHrefs(LAYOUT);
    const missing = faces.filter((url) => !preloaded.includes(url));
    expect(
      missing,
      "tools/first-load.mjs does not follow url(...) inside CSS, so a face with no " +
        "<link rel=preload> in BaseLayout is a download the size budget cannot see — and " +
        "one that can be fetched after e2e/zero-requests.spec.ts starts asserting silence",
    ).toEqual([]);
  });
});
