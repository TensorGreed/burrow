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
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { HARNESS_DIR } from "./build-output.js";
import {
  FORBIDDEN_CLAIMS,
  PAGE_MARKS,
  REDACT_MARKS,
  SECTION_7_MARKS,
  VERIFIES_MARK,
} from "./redact-copy.js";

const PAGE = "redact-pdf/index.html";

const ADR_0029 = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../../docs/adr/0029-what-redaction-does-and-what-it-refuses-to-do.md",
);

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
export function visibleParts(html: string): { title: string; description: string; body: string } {
  const title = html.match(/<title\b[^>]*>([\s\S]*?)<\/title>/i)?.[1] ?? "";
  // Attribute order is the serializer's choice, so find the tag first and its content second.
  const meta = html.match(/<meta\b[^>]*\bname="description"[^>]*>/i)?.[0] ?? "";
  const description = meta.match(/\bcontent="([^"]*)"/i)?.[1] ?? "";
  const body = (html.match(/<body[^>]*>([\s\S]*)<\/body>/i)?.[1] ?? "")
    .replace(/<!--[\s\S]*?-->/g, " ")
    .replace(/<script\b[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[\s\S]*?<\/style>/gi, " ")
    .replace(/<[^>]*>/g, "");
  return { title: decode(title), description: decode(description), body: decode(body) };
}

export function visibleText(html: string): string {
  const { title, description, body } = visibleParts(html);
  return [title, description, body].join(" ").replace(/\s+/g, " ").trim();
}

/** HTML entities to characters, and whitespace to single spaces. */
function decode(text: string): string {
  return text
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

/**
 * ADR 0029 §7 as prose: the section cut from its heading to §8's, with quote markers, emphasis
 * and code ticks removed and whitespace collapsed, so a mark is compared with the words.
 */
export function section7(adr: string): string {
  const start = adr.indexOf("\n### 7.");
  const end = adr.indexOf("\n### 8.");
  if (start < 0 || end < start) return "";
  return adr
    .slice(start, end)
    .replace(/^>\s?/gm, "")
    .replace(/\*\*|\*|`/g, "")
    .replace(/\s+/g, " ");
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
    // Attribute order does not hide the description.
    expect(visibleParts('<meta content="D two" name="description"><body></body>').description).toBe(
      "D two",
    );
  });

  it("§7 is cut from the ADR as prose, and a mark that drifted from it is not found", () => {
    const adr =
      "## x\n\n### 7. Copy\n\n> **Three** things are `outside`\n> a page.\n\n### 8. Next\n\nfour";
    // The drifted wording the review planted in ALT_TEXT_MARK is not in the section.
    expect(section7(adr).includes("Three things are outside, some of it.")).toBe(false);
    expect(section7(adr).includes("Three things are outside a page.")).toBe(true);
    expect(section7(adr).includes("four")).toBe(false);
    expect(section7("no sections here")).toBe("");
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
    // The wider wordings the review of S1 found passing the narrower rules (2026-10-14).
    for (const [sentence, name] of [
      ["Pictures in the area are redacted too.", "pictures or shapes are redacted"],
      ["Images in the area are blacked out as well.", "pictures or shapes are redacted"],
      ["Your text is removed for good.", "gone for good"],
      ["Your file is now safe to share.", "safe to share"],
      ["Nothing you removed can be recovered.", "irreversible"],
      // D9 (owner, 2026-10-19): a re-fed result carries no verdict on the run it came from.
      ["Your last result is already verified.", "the earlier redaction was verified"],
      ["Redact another area of this verified result.", "the earlier redaction was verified"],
      ["The previous run was checked.", "the earlier redaction was verified"],
    ] as const) {
      expect(claimsMade(sentence), sentence).toContain(name);
    }
    // No rule may match everything: a neutral sentence trips none of them.
    expect(claimsMade("The text you selected is removed from the page.")).toEqual([]);
    // Nor may the D9 rule catch the page's own wording for a re-fed result (near misses).
    expect(claimsMade("Redact another area of this result")).toEqual([]);
    expect(claimsMade("Not Only PDF checked the result and would not hand it over.")).toEqual([]);
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

  const html = existsSync(path) ? readFileSync(path, "utf8") : "";
  const text = visibleText(html);

  it("yields a title, a description and a body, so the rules below read all three", () => {
    const parts = visibleParts(html);
    expect(parts.title, "no title was extracted").not.toBe("");
    expect(parts.description, "no meta description was extracted").not.toBe("");
    expect(parts.body, "no body text was extracted").not.toBe("");
  });

  it("quotes every §7 mark from ADR 0029 §7 itself, so the module cannot drift from it", () => {
    const section = section7(readFileSync(ADR_0029, "utf8"));
    expect(section, "§7 was not found in ADR 0029").not.toBe("");
    // ELEVEN: §7's own sentences. The four page marks are written for the page, not quoted.
    expect(SECTION_7_MARKS).toHaveLength(11);
    expect(PAGE_MARKS).toHaveLength(4);
    expect(SECTION_7_MARKS.filter((mark) => !section.includes(mark))).toEqual([]);
  });

  it("carries every ADR 0029 §7 mark", () => {
    // THE COUNT IS THE MEASUREMENT: fifteen marks, so a list that quietly shrank would pass
    // the loop below over less copy than §7 requires.
    expect(REDACT_MARKS).toHaveLength(15);
    expect(missingMarks(text)).toEqual([]);
  });

  it('says "Not Only PDF", and never "burrow", in what a person reads', () => {
    expect(text).toContain("Not Only PDF");
    expect(namesBurrow(text)).toBe(false);
  });

  it("makes none of the forbidden claims", () => {
    expect(FORBIDDEN_CLAIMS).toHaveLength(15);
    expect(claimsMade(text)).toEqual([]);
    // The exemption is used exactly where §7 puts it, not as a blanket pass.
    expect(text.split(VERIFIES_MARK)).toHaveLength(2);
  });
});
