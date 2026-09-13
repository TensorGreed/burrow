// The rules `tokens.test.ts` enforces over `tokens.css`, as pure functions.
//
// WHY THIS IS A MODULE AND NOT JUST ASSERTIONS IN THE TEST
//
// A rule that lives inside a test is exercised only against the real file, which passes.
// Nobody has ever seen it fail, so nobody knows it can. Every rule-driven check in this
// repository has at some point contained a rule that matched nothing while the output
// reported a healthy count -- `check-no-generated-files.sh` with 15 of 16 patterns
// truncated to `(^`, `detect-engine-components.py` rejecting the example in its own
// comment, `check-engine-licences.py` comparing 4 of 15 texts. See CLAUDE.md.
//
// So the rules are functions, and `tokens.test.ts` feeds each one a fixture it must accept
// AND a near-miss it must reject, on every run, with the rejection naming its reason.
// Parsing is separated from judging for the same purpose: a parser that silently returns
// nothing makes every downstream rule vacuously true, and that is the failure this file is
// shaped to make visible rather than the one it is shaped to avoid.

/** A parsed custom-property block: property name (without `--`) to value, in source order. */
export type Tokens = Map<string, string>;

/** Both themes, as they are declared in `tokens.css`. */
export interface Themes {
  light: Tokens;
  dark: Tokens;
}

/**
 * Pull the `:root` declarations out of a stylesheet, for the light theme and for the
 * `prefers-color-scheme: dark` override.
 *
 * Throws rather than returning empty on a stylesheet it does not recognise. An empty parse
 * is indistinguishable from a clean file to every rule below, and "the check examined
 * nothing" must not be able to read as "the check passed".
 */
export function parseThemes(css: string): Themes {
  const dark = firstBlock(css, /@media\s*\(\s*prefers-color-scheme:\s*dark\s*\)\s*\{/);
  // Remove the dark block before looking for the base `:root`, so its inner `:root` cannot
  // be mistaken for the light one.
  const withoutDark = dark === null ? css : css.slice(0, dark.start) + css.slice(dark.end);

  const light = firstBlock(withoutDark, /(^|\})\s*:root\s*\{/m);
  if (light === null) {
    throw new Error("tokens.css: no `:root { ... }` block; nothing to check");
  }
  if (dark === null) {
    throw new Error("tokens.css: no `prefers-color-scheme: dark` block; nothing to check");
  }

  const lightTokens = declarations(light.body);
  const darkTokens = declarations(firstBlock(dark.body, /:root\s*\{/)?.body ?? "");

  if (lightTokens.size === 0) {
    throw new Error("tokens.css: the `:root` block declares no custom properties");
  }
  if (darkTokens.size === 0) {
    throw new Error("tokens.css: the dark `:root` block declares no custom properties");
  }

  return { light: lightTokens, dark: darkTokens };
}

/** The body of the first brace-balanced block whose opener matches `opener`. */
function firstBlock(
  source: string,
  opener: RegExp,
): { body: string; start: number; end: number } | null {
  const match = opener.exec(source);
  if (match === null) return null;
  const open = source.indexOf("{", match.index);
  if (open === -1) return null;

  let depth = 0;
  for (let i = open; i < source.length; i += 1) {
    const ch = source[i];
    if (ch === "{") depth += 1;
    else if (ch === "}") {
      depth -= 1;
      if (depth === 0) {
        return { body: source.slice(open + 1, i), start: match.index, end: i + 1 };
      }
    }
  }
  return null;
}

/** `--name: value;` pairs in a block body, comments stripped. */
function declarations(body: string): Tokens {
  const tokens: Tokens = new Map();
  const withoutComments = body.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const [, name, value] of withoutComments.matchAll(/--([\w-]+)\s*:\s*([^;]+);/g)) {
    tokens.set(name, value.trim());
  }
  return tokens;
}

/** Tokens whose value is a single `#rrggbb` literal. */
export function colourTokens(tokens: Tokens): Map<string, string> {
  const colours = new Map<string, string>();
  for (const [name, value] of tokens) {
    if (/^#[0-9a-fA-F]{6}$/.test(value)) colours.set(name, value.toLowerCase());
  }
  return colours;
}

/** Relative luminance, WCAG 2.1 §relative-luminance. */
export function luminance(hex: string): number {
  const m = /^#([0-9a-fA-F]{6})$/.exec(hex);
  if (m === null) throw new Error(`not a #rrggbb colour: ${hex}`);
  const channels = [0, 2, 4].map((i) => Number.parseInt(m[1].slice(i, i + 2), 16) / 255);
  const [r, g, b] = channels.map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio between two `#rrggbb` colours, 1..21. */
export function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/**
 * How each coloured token is used, which is what decides the ratio it must clear.
 *
 * Written out per token rather than inferred from the name. A heuristic ("anything called
 * `--rule` is decorative") would silently exempt the next token someone names badly, and
 * the exemption is the whole risk: `--rule` sits at 1.3:1, so being wrong about which list
 * a token belongs in is the difference between a passing check and an unreadable interface.
 */
export const TEXT_TOKENS = ["ink", "ink-quiet", "signal", "refuse"] as const;

/** Boundaries a person must be able to see, but which carry no text. WCAG 1.4.11. */
export const BOUNDARY_TOKENS = ["edge"] as const;

/** Purely decorative separators. Exempt from a ratio, and each one needs a reason. */
export const DECORATIVE_TOKENS: Record<string, string> = {
  rule: "separates blocks of content; never outlines a control and never carries text",
};

export const TEXT_MINIMUM = 4.5;
export const BOUNDARY_MINIMUM = 3;

export interface Finding {
  theme: string;
  token: string;
  ratio: number;
  required: number;
}

export interface Report {
  /** Every (theme, token) pair actually compared. The count is the measurement. */
  checked: Finding[];
  failures: Finding[];
  /** Tokens present in one theme and missing from the other, either way round. */
  unpaired: string[];
  /** Colour tokens in no list above — neither checked nor knowingly exempt. */
  unclassified: string[];
}

/**
 * Check every coloured token in both themes against the ground of its own theme.
 *
 * Reports what it examined rather than only whether it passed: `checked` is the list of
 * comparisons made, so a caller can gate on the expected count. A ratio check that quietly
 * compared nothing would otherwise look exactly like one that compared everything.
 */
export function checkContrast(themes: Themes): Report {
  const report: Report = { checked: [], failures: [], unpaired: [], unclassified: [] };

  const lightColours = colourTokens(themes.light);
  const darkColours = colourTokens(themes.dark);

  for (const name of lightColours.keys()) {
    if (!darkColours.has(name)) report.unpaired.push(name);
  }
  for (const name of darkColours.keys()) {
    if (!lightColours.has(name)) report.unpaired.push(name);
  }

  for (const [theme, colours] of [
    ["light", lightColours],
    ["dark", darkColours],
  ] as const) {
    const paper = colours.get("paper");
    if (paper === undefined) {
      throw new Error(`${theme}: no --paper token, so there is no ground to compare against`);
    }

    for (const token of colours.keys()) {
      if (token === "paper") continue;

      const required = (TEXT_TOKENS as readonly string[]).includes(token)
        ? TEXT_MINIMUM
        : (BOUNDARY_TOKENS as readonly string[]).includes(token)
          ? BOUNDARY_MINIMUM
          : null;

      if (required === null) {
        if (!(token in DECORATIVE_TOKENS)) report.unclassified.push(`${theme}/${token}`);
        continue;
      }

      const value = colours.get(token);
      if (value === undefined) continue;
      const finding: Finding = { theme, token, ratio: contrast(value, paper), required };
      report.checked.push(finding);
      if (finding.ratio < required) report.failures.push(finding);
    }
  }

  return report;
}

/**
 * Every `url(...)` inside an `@font-face` block in a stylesheet.
 *
 * Used to assert each one is preloaded from the layout — `tools/first-load.mjs` states
 * that it does not follow `url(...)` in CSS, so a face referenced only here is a download
 * the size budget cannot see.
 */
export function fontFaceUrls(css: string): string[] {
  const urls: string[] = [];
  for (const [, body] of css.matchAll(/@font-face\s*\{([\s\S]*?)\}/g)) {
    for (const [, url] of body.matchAll(/url\(\s*["']?([^"')]+)["']?\s*\)/g)) {
      urls.push(url.trim());
    }
  }
  return urls;
}

/** Every `href` on a `<link rel="preload">` in a piece of markup. */
export function preloadedHrefs(markup: string): string[] {
  const hrefs: string[] = [];
  for (const [, tag] of markup.matchAll(/<link\b([^>]*)>/g)) {
    if (!/\brel\s*=\s*["']preload["']/.test(tag)) continue;
    const href = /\bhref\s*=\s*["']([^"']+)["']/.exec(tag);
    if (href !== null) hrefs.push(href[1]);
  }
  return hrefs;
}
