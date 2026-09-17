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
export const BOUNDARY_TOKENS = ["edge", "accent"] as const;

/**
 * A token used as a FILL, mapped to the token used for text on top of it.
 *
 * `--accent` is not text on the ground and must not be checked as if it were: the ratio
 * that decides whether a person can read the primary action is `--accent-ink` against
 * `--accent`, which is a pair rather than a token. It is *also* in `BOUNDARY_TOKENS`,
 * because a filled control still has an extent somebody has to see against the paper.
 *
 * The ink token is deliberately not in `TEXT_TOKENS`: near-white on the light ground is
 * 1.06:1, and a rule that compared it against the paper would fail a correct palette —
 * which is how a check gets weakened to make a true thing pass.
 */
export const FILL_PAIRS: Record<string, string> = {
  accent: "accent-ink",
};

/** Purely decorative separators. Exempt from a ratio, and each one needs a reason. */
export const DECORATIVE_TOKENS: Record<string, string> = {
  rule: "separates blocks of content; never outlines a control and never carries text",
};

export const TEXT_MINIMUM = 4.5;
export const BOUNDARY_MINIMUM = 3;

/**
 * The three colours that MEAN something, and the perceptual distance any two of them must
 * keep. See `checkSemanticDistance`.
 */
export const SEMANTIC_TOKENS = ["signal", "refuse", "accent"] as const;

/**
 * CIEDE2000, and the bar was written down before the candidates were measured.
 *
 * 25 is not a round number picked to pass. `--signal` and `--refuse` — the two values this
 * palette already treated as un-confusable — measured **50.0** apart in the light theme and
 * **49.2** in the dark one *before `--refuse` moved*, so half of the distance this palette
 * had already accepted is the floor a third semantic value has to clear. Past tense, and the
 * bar is not re-derived from the current values: they now measure 56.2 and 59.9, and a floor
 * that rose every time the palette improved would be a ratchet rather than a rule. The first accent proposed for this palette, `#b4531a`
 * against the old brick `--refuse` `#8a2b1f`, measured **16.6** and did not clear it; the
 * refusal value moved to crimson rather than the bar moving to 16.
 */
export const SEMANTIC_MINIMUM_DELTA_E = 25;

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
        // Ink that only ever sits on a fill is checked by `checkFillPairs` against that
        // fill. Comparing it with the paper here would fail a correct palette — near-white
        // on the light ground is 1.06:1 — and the way that failure normally gets "fixed"
        // is by weakening the rule.
        const onAFill = Object.values(FILL_PAIRS).includes(token);
        if (!onAFill && !(token in DECORATIVE_TOKENS)) {
          report.unclassified.push(`${theme}/${token}`);
        }
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

export interface DistanceFinding {
  theme: string;
  pair: string;
  deltaE: number;
  required: number;
}

export interface DistanceReport {
  /** Every pair actually compared, in both themes. The count is the measurement. */
  checked: DistanceFinding[];
  failures: DistanceFinding[];
  /** Semantic tokens absent from a theme — a pair that was never compared at all. */
  missing: string[];
}

/**
 * No two of `--signal`, `--refuse` and `--accent` may be confusable, in either theme.
 *
 * **A wrong colour on a refusal is a correctness bug here, not a styling one.** The three
 * say different things — this was measured, this was refused, this is what you press — and
 * the whole reserved-colour system depends on a reader telling them apart at a glance
 * rather than by reading the label.
 *
 * Reports what it compared, not only whether it passed: a distance check that compared
 * nothing looks exactly like one that compared everything, which is the failure mode
 * `CLAUDE.md` names by measurement rather than by principle.
 */
export function checkSemanticDistance(themes: Themes): DistanceReport {
  const report: DistanceReport = { checked: [], failures: [], missing: [] };

  for (const [theme, tokens] of [
    ["light", themes.light],
    ["dark", themes.dark],
  ] as const) {
    const colours = colourTokens(tokens);
    const names = SEMANTIC_TOKENS as readonly string[];

    for (const name of names) {
      if (!colours.has(name)) report.missing.push(`${theme}/${name}`);
    }

    for (let i = 0; i < names.length; i += 1) {
      for (let j = i + 1; j < names.length; j += 1) {
        const a = colours.get(names[i]);
        const b = colours.get(names[j]);
        if (a === undefined || b === undefined) continue;
        const finding: DistanceFinding = {
          theme,
          pair: `${names[i]}/${names[j]}`,
          deltaE: deltaE2000(a, b),
          required: SEMANTIC_MINIMUM_DELTA_E,
        };
        report.checked.push(finding);
        if (finding.deltaE < finding.required) report.failures.push(finding);
      }
    }
  }

  return report;
}

/**
 * Text on a fill is checked against the fill, never against the paper.
 *
 * Its own function rather than a branch inside `checkContrast`, because the two answer
 * different questions and the counts must not be pooled: `checkContrast` gates on how many
 * tokens it compared against the ground, and a pair silently landing in that total would
 * make the expected count drift for a reason nobody could reconstruct.
 */
export function checkFillPairs(themes: Themes): Report {
  const report: Report = { checked: [], failures: [], unpaired: [], unclassified: [] };

  for (const [theme, tokens] of [
    ["light", themes.light],
    ["dark", themes.dark],
  ] as const) {
    const colours = colourTokens(tokens);
    for (const [fill, inkToken] of Object.entries(FILL_PAIRS)) {
      const fillValue = colours.get(fill);
      const inkValue = colours.get(inkToken);
      if (fillValue === undefined || inkValue === undefined) {
        report.unpaired.push(`${theme}/${fill}+${inkToken}`);
        continue;
      }
      const finding: Finding = {
        theme,
        token: `${inkToken} on ${fill}`,
        ratio: contrast(inkValue, fillValue),
        required: TEXT_MINIMUM,
      };
      report.checked.push(finding);
      if (finding.ratio < finding.required) report.failures.push(finding);
    }
  }

  return report;
}

/**
 * CIE L*a*b* under D65, from a `#rrggbb` colour.
 *
 * Separate from `luminance` above on purpose: WCAG luminance answers "can this be read",
 * and Lab answers "can these two be told apart", which are different questions with
 * different answers. `--accent` and `--refuse` can both be perfectly readable on the paper
 * and still be the same colour to a reader glancing at them.
 */
export function lab(hex: string): [number, number, number] {
  const m = /^#([0-9a-fA-F]{6})$/.exec(hex);
  if (m === null) throw new Error(`not a #rrggbb colour: ${hex}`);
  const [r, g, b] = [0, 2, 4]
    .map((i) => Number.parseInt(m[1].slice(i, i + 2), 16) / 255)
    .map((c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));

  const x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
  const y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
  const z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;

  const f = (t: number): number => (t > 0.008856 ? Math.cbrt(t) : 7.787 * t + 16 / 116);
  const [fx, fy, fz] = [f(x), f(y), f(z)];
  return [116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)];
}

/**
 * CIEDE2000 colour difference between two `#rrggbb` colours.
 *
 * The full formula rather than a Euclidean distance in Lab, because the whole reason this
 * check exists is the region of the wheel where Lab distance and perceived distance
 * disagree most — saturated warm reds and oranges, which is exactly where `--refuse` and
 * `--accent` both live.
 */
export function deltaE2000(a: string, b: string): number {
  const [l1, a1, b1] = lab(a);
  const [l2, a2, b2] = lab(b);
  const rad = (deg: number): number => (deg * Math.PI) / 180;
  const deg = (r: number): number => ((r * 180) / Math.PI + 360) % 360;

  const c1 = Math.hypot(a1, b1);
  const c2 = Math.hypot(a2, b2);
  const cBar = (c1 + c2) / 2;
  const g = 0.5 * (1 - Math.sqrt(cBar ** 7 / (cBar ** 7 + 25 ** 7)));
  const a1p = (1 + g) * a1;
  const a2p = (1 + g) * a2;
  const c1p = Math.hypot(a1p, b1);
  const c2p = Math.hypot(a2p, b2);
  const h1p = c1p === 0 ? 0 : deg(Math.atan2(b1, a1p));
  const h2p = c2p === 0 ? 0 : deg(Math.atan2(b2, a2p));

  const dLp = l2 - l1;
  const dCp = c2p - c1p;
  const dhp = c1p * c2p === 0 ? 0 : ((((h2p - h1p + 180) % 360) + 360) % 360) - 180;
  const dHp = 2 * Math.sqrt(c1p * c2p) * Math.sin(rad(dhp) / 2);

  const lBar = (l1 + l2) / 2;
  const cBarP = (c1p + c2p) / 2;
  let hBar: number;
  if (c1p * c2p === 0) hBar = h1p + h2p;
  else if (Math.abs(h1p - h2p) <= 180) hBar = (h1p + h2p) / 2;
  else if (h1p + h2p < 360) hBar = (h1p + h2p + 360) / 2;
  else hBar = (h1p + h2p - 360) / 2;

  const t =
    1 -
    0.17 * Math.cos(rad(hBar - 30)) +
    0.24 * Math.cos(rad(2 * hBar)) +
    0.32 * Math.cos(rad(3 * hBar + 6)) -
    0.2 * Math.cos(rad(4 * hBar - 63));
  const sL = 1 + (0.015 * (lBar - 50) ** 2) / Math.sqrt(20 + (lBar - 50) ** 2);
  const sC = 1 + 0.045 * cBarP;
  const sH = 1 + 0.015 * cBarP * t;
  const rT =
    -Math.sin(rad(60 * Math.exp(-(((hBar - 275) / 25) ** 2)))) *
    2 *
    Math.sqrt(cBarP ** 7 / (cBarP ** 7 + 25 ** 7));

  return Math.sqrt(
    (dLp / sL) ** 2 + (dCp / sC) ** 2 + (dHp / sH) ** 2 + rT * (dCp / sC) * (dHp / sH),
  );
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
