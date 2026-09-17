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
  FILL_PAIRS,
  SEMANTIC_MINIMUM_DELTA_E,
  SEMANTIC_TOKENS,
  TEXT_MINIMUM,
  TEXT_TOKENS,
  checkContrast,
  checkFillPairs,
  checkSemanticDistance,
  colourTokens,
  contrast,
  deltaE2000,
  fontFaceUrls,
  luminance,
  parseThemes,
  preloadedHrefs,
} from "./tokens-check.js";

const WEB = join(REPO, "apps", "web");
const read = (relative: string) => readFileSync(join(WEB, relative), "utf8");

/**
 * Every stylesheet that can reach a token, so a rule about where a colour may appear is not
 * asserted over one file out of eight. The islands are `.svelte` with a `<style>` block and
 * the pages are `.astro` with one; both are read whole, which is coarse and errs towards
 * scanning more than the style block rather than less.
 */
const COMPONENT_SOURCES: [string, string][] = [
  "src/components/MergeTool.svelte",
  "src/components/SplitTool.svelte",
  "src/components/RotateTool.svelte",
  "src/components/ReorderTool.svelte",
  "src/components/CompressTool.svelte",
  "src/components/PageThumbnails.svelte",
].map((f) => [f, read(f)]);

const TOKENS_CSS = read("src/styles/tokens.css");
const BASE_CSS = read("src/styles/base.css");
const LAYOUT = read("src/layouts/BaseLayout.astro");

const STYLED_SOURCES: [string, string][] = [
  ["src/styles/base.css", BASE_CSS],
  ["src/styles/tokens.css", TOKENS_CSS],
  ...COMPONENT_SOURCES,
  ["src/pages/index.astro", read("src/pages/index.astro")],
  ["src/pages/how-it-works.astro", read("src/pages/how-it-works.astro")],
  ["src/pages/credits.astro", read("src/pages/credits.astro")],
];

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

describe("the distance rule itself", () => {
  // The rule that says three reserved colours cannot be mistaken for one another. Pointed
  // at fixtures first, because its verdict on the real palette is worth nothing until it
  // has been seen to fail.

  it("computes the CIEDE2000 anchors", () => {
    expect(deltaE2000("#ffffff", "#ffffff")).toBeCloseTo(0, 10);
    // THE HISTORICAL PAIR, pinned by value rather than by token, because it is where the bar
    // came from: `--signal` against the BRICK `--refuse` this palette shipped with. Pinning
    // the live tokens instead would have pinned 56.2 with a `> 50` assertion, leaving six
    // points of silent drift under a comment claiming to protect the derivation.
    expect(deltaE2000("#0b6e5e", "#8a2b1f")).toBeCloseTo(50.0, 1);
  });

  it("rejects the accent that was proposed before it was measured", () => {
    // #b4531a against the OLD brick `--refuse`. This is the pair the rule exists for: both
    // clear every contrast minimum, both are legible, and they are the same colour to
    // somebody glancing at the page. Measured at 16.6.
    const measured = deltaE2000("#b4531a", "#8a2b1f");
    expect(measured).toBeLessThan(SEMANTIC_MINIMUM_DELTA_E);

    const themes = {
      light: new Map([
        ["paper", "#f2f4f5"],
        ["signal", "#0b6e5e"],
        ["refuse", "#8a2b1f"],
        ["accent", "#b4531a"],
      ]),
      dark: new Map([
        ["paper", "#12161a"],
        ["signal", "#5fd3bb"],
        ["refuse", "#f09a88"],
        ["accent", "#e58a43"],
      ]),
    };
    const report = checkSemanticDistance(themes);
    expect(report.checked).toHaveLength(6);
    expect(report.failures.map((f) => `${f.theme}/${f.pair}`)).toEqual([
      "light/refuse/accent",
      "dark/refuse/accent",
    ]);
  });

  it("says what it could not compare rather than passing it", () => {
    // A palette with no accent at all must not read as a palette whose accent is distinct.
    const themes = {
      light: new Map([
        ["paper", "#f2f4f5"],
        ["signal", "#0b6e5e"],
        ["refuse", "#8f1733"],
      ]),
      dark: new Map([
        ["paper", "#12161a"],
        ["signal", "#5fd3bb"],
        ["refuse", "#f5879f"],
      ]),
    };
    const report = checkSemanticDistance(themes);
    expect(report.missing).toEqual(["light/accent", "dark/accent"]);
    expect(report.checked).toHaveLength(2);
    expect(report.failures).toEqual([]);
  });
});

describe("the fill-pair rule itself", () => {
  it("accepts ink that is readable on its fill and rejects ink that is not", () => {
    const good = {
      light: new Map([
        ["paper", "#f2f4f5"],
        ["accent", "#b4531a"],
        ["accent-ink", "#fffaf6"],
      ]),
      dark: new Map([
        ["paper", "#12161a"],
        ["accent", "#e58a43"],
        ["accent-ink", "#12161a"],
      ]),
    };
    expect(checkFillPairs(good).failures).toEqual([]);
    expect(checkFillPairs(good).checked).toHaveLength(2);

    // The near-miss is the mistake somebody actually makes: setting the label on a filled
    // button to the body ink, which is unreadable on a mid-tone fill in one theme and fine
    // in the other — so a rule that checked only one theme would pass it.
    const bad = {
      light: new Map([
        ["paper", "#f2f4f5"],
        ["accent", "#b4531a"],
        ["accent-ink", "#14181c"],
      ]),
      dark: good.dark,
    };
    const report = checkFillPairs(bad);
    expect(report.failures.map((f) => f.theme)).toEqual(["light"]);
    expect(report.checked).toHaveLength(2);
  });

  it("reports a fill whose ink token is missing instead of skipping it", () => {
    const themes = {
      light: new Map([
        ["paper", "#f2f4f5"],
        ["accent", "#b4531a"],
      ]),
      dark: new Map([
        ["paper", "#12161a"],
        ["accent", "#e58a43"],
      ]),
    };
    const report = checkFillPairs(themes);
    expect(report.checked).toEqual([]);
    expect(report.unpaired).toEqual(["light/accent+accent-ink", "dark/accent+accent-ink"]);
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
    // `+ 1` is the ground; the fill inks are checked against their fill by
    // `checkFillPairs`, not against the paper, so they are counted here and compared there.
    expect(colours.size).toBe(
      perTheme + 1 + Object.keys(DECORATIVE_TOKENS).length + Object.keys(FILL_PAIRS).length,
    );
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
    expect([...BOUNDARY_TOKENS].sort()).toEqual(["accent", "edge"]);
    expect([...SEMANTIC_TOKENS].sort()).toEqual(["accent", "refuse", "signal"]);
    expect(Object.entries(FILL_PAIRS)).toEqual([["accent", "accent-ink"]]);
  });

  it("keeps every pair of reserved colours apart, in both themes, and says how many", () => {
    const distance = checkSemanticDistance(themes);
    // Three colours, three pairs, two themes. The count is the measurement.
    expect(distance.missing).toEqual([]);
    expect(distance.checked).toHaveLength(6);

    const failures = distance.failures.map(
      (f) => `${f.theme}/${f.pair}: ΔE ${f.deltaE.toFixed(1)}, needs ${f.required}`,
    );
    expect(failures).toEqual([]);
  });

  it("makes the primary action's label readable on the primary action's fill", () => {
    const fills = checkFillPairs(themes);
    expect(fills.unpaired).toEqual([]);
    expect(fills.checked).toHaveLength(2);
    expect(fills.failures).toEqual([]);
  });

  it("carries a refusal with more than its colour", () => {
    // COLOUR ALONE DOES NOT SURVIVE COLOUR-VISION DEFICIENCY OR GREYSCALE, and the distance
    // rule above says nothing about that. `--accent` and `--refuse` clear 25 in ordinary
    // vision and collapse under Viénot-Brettel-Mollon: in the LIGHT theme to 18.0
    // (deuteranopia), 12.5 (tritanopia) and 13.7 (greyscale); in the DARK theme to 5.1 and
    // 2.4, which is the same colour. The dark pair is what decides this. So a refusal must
    // not be a colour and nothing else. ADR 0028.
    const rule = /\.refusal\s*\{([^}]*)\}/.exec(BASE_CSS);
    expect(rule).not.toBeNull();
    expect(rule?.[1]).toContain("var(--refuse)");
    // THE WEIGHT, NOT MERELY A `font-weight` DECLARATION. `toContain("font-weight")` was
    // satisfied by `var(--weight-body)`, so the mutation that made the refusal exactly as
    // heavy as the prose around it survived the whole suite — a defence asserted by the
    // presence of a property name rather than by its value.
    expect(rule?.[1]).toMatch(/font-weight:\s*var\(--weight-strong\)/);

    // And the BLOCK form, which is the other four islands. A refusal that is a whole
    // `role="alert"` paragraph is carried by a bar rather than by weight; `.refusal` covered
    // one island in five until this was noticed.
    const block = /\.notice,\s*\.choice__problem,\s*\.file__problem\s*\{([^}]*)\}/.exec(BASE_CSS);
    expect(block, "block refusals need a carrier that is not the hue").not.toBeNull();
    expect(block?.[1]).toMatch(/border-inline-start:\s*2px solid var\(--refuse\)/);
  });

  it("never fills anything with the refusal colour", () => {
    // The structural half of the same defence: `--accent` is only ever a fill and
    // `--refuse` is only ever text or a border, so a filled control and a bold phrase stay
    // distinguishable with no colour at all. A `background: var(--refuse)` would put the
    // two in the same form, where only the hue separates them.
    // OVER EVERY STYLESHEET, not just this one. `--refuse` and `--accent` are reachable from
    // the five islands' scoped styles and the page styles too, which is the likelier place
    // for a violation — and a structural rule asserted over one of eight stylesheets is this
    // repository's "4 of 15" in the rule that does the real work in greyscale.
    expect(STYLED_SOURCES.length, "the scan must examine every stylesheet").toBe(11);
    for (const [name, source] of STYLED_SOURCES) {
      expect(source, `${name} fills something with --refuse`).not.toMatch(
        /background[^;]*var\(--refuse\)/,
      );
    }
    expect(BASE_CSS).toMatch(/background:\s*var\(--accent\)/);
  });

  it("keeps the two site-wide rules that were inert when they lived in one island", () => {
    // `.visually-hidden` and `.drop:focus-within` were defined in `MergeTool.svelte` and used
    // by four other components, where Svelte's scoping made them do nothing: four tool
    // headings and four `aria-live` regions were visible, four file inputs were not hidden,
    // and four drop zones had no focus ring. That shipped for months and nothing failed.
    //
    // CLAUDE.md: "a habit that has failed twice is not a control". This is the control, and
    // it has two halves — the rule must exist HERE, and no component may define it again,
    // because the redefinition is what made it scoped and inert in the first place.
    expect(BASE_CSS, ".visually-hidden belongs to the site").toMatch(/\.visually-hidden\s*\{/);
    expect(BASE_CSS, "the drop zone's focus ring belongs to the site").toMatch(
      /\.drop:focus-within\s*\{/,
    );

    const redefined = COMPONENT_SOURCES.filter(([, source]) =>
      /\.visually-hidden\s*\{/.test(source),
    ).map(([name]) => name);
    expect(redefined, "a component redefining this makes it scoped, which is the bug").toEqual([]);
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
