// The held `/redact-pdf` page says what ADR 0029 §7 says, and nothing it forbids (#136, S1).
//
// Read off the HARNESS build, because that is the only build the page is in: it is held, and
// `production-build.test.ts` asserts it is absent from the production one.
//
// Three rules, each run against fixtures before it is run against the build, so a rule that
// matched nothing cannot read like a page that is clean:
//
//   * every mark in `REDACT_MARKS` is in the page's visible text, gated on the mark count;
//   * "burrow" is not in the visible text -- the product is Not Only PDF -- while it may still
//     be in an href, an attribute, a comment or a script, which a person does not read;
//   * no `FORBIDDEN_CLAIMS` pattern matches the visible text, with the one negated sentence
//     §7 requires (`VERIFIES_MARK`) taken out first.

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { HARNESS_DIR } from "./build-output.js";
import { FORBIDDEN_CLAIMS, REDACT_MARKS, VERIFIES_MARK } from "./redact-copy.js";

const PAGE = "redact-pdf/index.html";

const ENTITIES: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
  nbsp: " ",
  ldquo: "“",
  rdquo: "”",
  lsquo: "‘",
  rsquo: "’",
  mdash: "—",
  ndash: "–",
  rarr: "→",
};

/**
 * The text a person reads: the document title, the meta description (it is what a search
 * result shows), and the body with comments, scripts, styles and every tag removed.
 *
 * Attributes go with the tags, so the masthead's `href` to the repository does not count as
 * prose -- it is a link target, and its visible text is "Source".
 */
export function visibleText(html: string): string {
  const title = html.match(/<title>([\s\S]*?)<\/title>/i)?.[1] ?? "";
  const description = html.match(/<meta\s+name="description"\s+content="([^"]*)"/i)?.[1] ?? "";
  const body = (html.match(/<body[^>]*>([\s\S]*)<\/body>/i)?.[1] ?? "")
    .replace(/<!--[\s\S]*?-->/g, " ")
    .replace(/<script\b[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]*>/g, "");
  return [title, description, body]
    .join(" ")
    .replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (whole, name: string) => {
      if (name[0] === "#") {
        const code =
          name[1].toLowerCase() === "x" ? parseInt(name.slice(2), 16) : Number(name.slice(1));
        return String.fromCodePoint(code);
      }
      return ENTITIES[name.toLowerCase()] ?? whole;
    })
    .replace(/\s+/g, " ")
    .trim();
}

/** The marks a text is missing. */
export const missingMarks = (text: string): string[] =>
  REDACT_MARKS.filter((mark) => !text.includes(mark));

/** Whether a text names the repository rather than the product. */
export const namesBurrow = (text: string): boolean => /burrow/i.test(text);

/** The forbidden claims a text makes, once §7's one negated sentence is taken out. */
export const claimsMade = (text: string): string[] => {
  const without = text.split(VERIFIES_MARK).join(" ");
  return FORBIDDEN_CLAIMS.filter(({ pattern }) => pattern.test(without)).map(({ name }) => name);
};

describe("the rules themselves", () => {
  it("visible text drops tags, attributes, comments and scripts, and keeps title and description", () => {
    const html =
      '<html><head><title>T one</title><meta name="description" content="D &amp; two">' +
      '</head><body><a href="https://github.com/TensorGreed/burrow">Source</a>' +
      "<!-- burrow --><script>const burrow = 1;</script><style>.burrow{}</style>" +
      "<p>Body &mdash; <strong>three</strong></p></body></html>";
    const text = visibleText(html);
    expect(text).toBe("T one D & two Source Body — three");
    expect(namesBurrow(text), "an href, a comment, a script or a style is not prose").toBe(false);
  });

  it('"burrow" in visible prose is caught, in any case, in the title as well as the body', () => {
    expect(namesBurrow(visibleText("<body><p>Made by burrow.</p></body>"))).toBe(true);
    expect(namesBurrow(visibleText("<title>Burrow</title><body></body>"))).toBe(true);
    // NEAR MISS: the product's name, and a word that merely shares letters.
    expect(namesBurrow(visibleText("<body><p>Not Only PDF. A borrowed idea.</p></body>"))).toBe(
      false,
    );
  });

  it("a page missing any one mark is reported as missing exactly that mark", () => {
    const all = REDACT_MARKS.join(" ");
    expect(missingMarks(all)).toEqual([]);
    for (const mark of REDACT_MARKS) {
      const without = REDACT_MARKS.filter((m) => m !== mark).join(" ");
      expect(missingMarks(without), `dropping "${mark}" was not noticed`).toEqual([mark]);
    }
  });

  it("the marks are distinct, and none is a substring of another", () => {
    // A mark contained in another would be "present" on a page that carried only the longer
    // one, and the dropping probe above would not see it.
    for (const a of REDACT_MARKS) {
      for (const b of REDACT_MARKS) {
        if (a !== b) expect(b.includes(a), `"${a}" is inside "${b}"`).toBe(false);
      }
    }
  });

  it("every forbidden claim catches its own fixture and passes its near miss", () => {
    for (const { name, pattern, caught, nearMiss } of FORBIDDEN_CLAIMS) {
      expect(pattern.test(caught), `${name} did not catch: ${caught}`).toBe(true);
      expect(pattern.test(nearMiss), `${name} caught its near miss: ${nearMiss}`).toBe(false);
      expect(claimsMade(caught), `${name} is not reported by name`).toContain(name);
    }
    // No rule may match everything: a neutral sentence trips none of them.
    expect(claimsMade("The text you selected is removed from the page.")).toEqual([]);
  });

  it("the §7 sentence that negates 'your secret is gone' is exempt, and only that sentence", () => {
    expect(claimsMade(`And ${VERIFIES_MARK}.`)).toEqual([]);
    // The same words, not negated, are a claim.
    expect(claimsMade("And it verifies that your secret is gone.")).toContain("the secret is gone");
  });
});

describe("the held /redact-pdf page, as built", () => {
  const path = join(HARNESS_DIR, PAGE);

  it("is in the harness build, so the rules below examine something", () => {
    expect(existsSync(path), `${PAGE} is not in the harness build`).toBe(true);
  });

  const text = existsSync(path) ? visibleText(readFileSync(path, "utf8")) : "";

  it("carries every ADR 0029 §7 mark", () => {
    // THE COUNT IS THE MEASUREMENT: thirteen marks, so a list that quietly shrank would pass
    // the loop below over less copy than §7 requires.
    expect(REDACT_MARKS).toHaveLength(13);
    expect(missingMarks(text)).toEqual([]);
  });

  it('says "Not Only PDF", and never "burrow", in what a person reads', () => {
    expect(text).toContain("Not Only PDF");
    expect(namesBurrow(text)).toBe(false);
  });

  it("makes none of the forbidden claims", () => {
    expect(FORBIDDEN_CLAIMS).toHaveLength(13);
    expect(claimsMade(text)).toEqual([]);
    // The exemption is used exactly where §7 puts it, not as a blanket pass.
    expect(text.split(VERIFIES_MARK)).toHaveLength(2);
  });
});
