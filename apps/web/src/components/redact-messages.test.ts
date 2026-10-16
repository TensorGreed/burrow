import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { FORBIDDEN_CLAIMS } from "../redact-copy.js";
import {
  CATEGORIES,
  RULES,
  type Category,
  messageFor,
  messageForRule,
  resultNotes,
  ruleOf,
} from "./redact-messages.js";

const REPO = fileURLToPath(new URL("../../../../", import.meta.url));

/**
 * Every refusal rule id the engine lists, from the committed copy of its table.
 *
 * THE ENGINE'S OWN LIST, NEVER A PARSE OF THE SOURCE. `burrow_engines::redact::rules::RULE_IDS`
 * is held by Rust tests to the production source: every id bracketed after a message prefix
 * (`"<lowercase words> [`), of any shape; every hyphenated `[id]` in a string literal; the names
 * of `Refusal`, `FrameRule` and `InkKind`; and `outcomes.tsv`. They also refuse any id interpolated
 * into a bracket, outside three whole literals each met exactly once: two that stamp a table's name
 * and one that only reads a refusal's prefix. `tests/redaction/rule-ids.txt` is held to the table,
 * and this test reads that file. What the Rust side cannot see, until #360's enum: an id built with
 * `concat!`, one after an interpolated prefix (`"{PREFIX} [word]"`), and one passed as a format
 * argument.
 */
function engineRules(): string[] {
  return readFileSync(join(REPO, "tests/redaction/rule-ids.txt"), "utf8")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "" && !line.startsWith("#"));
}

/** Every kind the binding can hand a page. */
const KINDS = [
  "PasswordRequired",
  "LimitExceeded",
  "OutputRejected",
  "Malformed",
  "Unsupported",
  "InvalidArgument",
  "EngineUnavailable",
  "InputFailed",
  "Io",
  "Internal",
];

/** Every string this module can put on the page, for the copy checks below. */
function everySentence(): string[] {
  return [
    ...Object.values(CATEGORIES).flatMap(({ title, next }) => [title, next]),
    ...KINDS.flatMap((kind) => {
      const { title, next } = messageFor({ kind });
      return [title, next];
    }),
    ...[
      "max_input_bytes",
      "max_pages",
      "max_duration_ms",
      "max_memory_bytes",
      "max_pixels",
    ].flatMap((limit) => {
      const { title, next } = messageFor({
        kind: "LimitExceeded",
        limit,
        requested: "9000000",
        allowed: "100",
      });
      return [title, next];
    }),
    ...resultNotes(1, 1),
    ...resultNotes(3, 2),
  ];
}

/**
 * The visible text of `RedactTool.svelte`: its markup's text nodes and its script's string
 * literals, comments taken out. What the island can put on screen besides this module's strings.
 */
function islandStrings(): string[] {
  const source = readFileSync(new URL("./RedactTool.svelte", import.meta.url), "utf8");
  const script = source.slice(source.indexOf("<script"), source.indexOf("</script>"));
  const markup = source
    .slice(source.indexOf("</script>") + "</script>".length, source.indexOf("<style"))
    .replace(/<!--[\s\S]*?-->/g, " ");
  const code = script.replace(/\/\*[\s\S]*?\*\//g, " ").replace(/^\s*\/\/.*$/gm, " ");
  const literals = [...code.matchAll(/"([^"\n]{12,})"|`([^`\n]{12,})`/g)].map(
    (m) => m[1] ?? m[2] ?? "",
  );
  const text = markup
    .replace(/\{[^{}]*\}/g, " ")
    .replace(/<[^>]*>/g, "\n")
    .split("\n")
    .map((line) => line.replace(/\s+/g, " ").trim())
    .filter((line) => /[a-z]{3}/i.test(line));
  return [...literals, ...text];
}

function claimsIn(text: string): string[] {
  return FORBIDDEN_CLAIMS.filter(({ pattern }) => pattern.test(text)).map(({ name }) => name);
}

describe("redaction's refusal categories (#136, S3 and S7)", () => {
  it("places every rule the engine lists in a category", () => {
    const listed = engineRules();
    // A COUNT, so a list that read nothing does not pass as "nothing unplaced".
    expect(listed.length, "the engine's rule table was not read").toBeGreaterThan(100);
    const unplaced = listed.filter((id) => !Object.hasOwn(RULES, id));
    expect(unplaced, `rules with no category: ${unplaced.join(", ")}`).toEqual([]);
  });

  it("places no rule the engine does not list", () => {
    const listed = new Set(engineRules());
    const stale = Object.keys(RULES).filter((id) => !listed.has(id));
    expect(stale, `categorised rules the engine does not list: ${stale.join(", ")}`).toEqual([]);
    expect(Object.keys(RULES).length).toBe(listed.size);
  });

  it("uses between six and ten categories, each with a title and a next step", () => {
    const used = new Set(Object.values(RULES));
    const all = Object.keys(CATEGORIES) as Category[];
    expect(all.length).toBeGreaterThanOrEqual(6);
    expect(all.length).toBeLessThanOrEqual(10);
    for (const category of all) {
      expect(used.has(category), `${category} has no rule`).toBe(true);
      const { title, next } = CATEGORIES[category];
      expect(title.endsWith("."), `${category}'s title`).toBe(true);
      expect(next.endsWith("."), `${category}'s next step`).toBe(true);
    }
  });

  it("makes none of the page's forbidden claims and never names burrow", () => {
    for (const text of everySentence()) {
      expect(text.toLowerCase(), text).not.toContain("burrow");
      for (const banned of ["sorry", "oops", "please", "an error occurred", "never"]) {
        expect(text.toLowerCase(), text).not.toContain(banned);
      }
      expect(claimsIn(text), text).toEqual([]);
    }
  });

  it("puts nothing on screen from the island itself that makes a forbidden claim (L3)", () => {
    const strings = islandStrings();
    // A COUNT, so a parse that found nothing does not pass.
    expect(strings.length, "the island's visible strings were not read").toBeGreaterThan(10);
    expect(strings.some((s) => s.includes("Remove the text"))).toBe(true);
    for (const text of strings) {
      expect(text.toLowerCase(), text).not.toContain("burrow");
      expect(claimsIn(text), text).toEqual([]);
    }
  });

  it("reads a rule id only from the FIRST bracket, and only if it is one of its own (L1)", () => {
    expect(ruleOf("pdf redaction [image-in-region]: an image")).toBe("image-in-region");
    expect(ruleOf("pdf region [rotate]: a /Rotate that is not a right angle")).toBe("rotate");
    // A known rule in a LATER bracket is not the message's rule, as Rust's FrameRule::of reads it.
    expect(ruleOf("[made-up-thing] then [font-flags]")).toBe("");
    // A FIRST BRACKET THAT DOES NOT CLOSE before a rule is no rule either (round 2's A).
    expect(ruleOf("pdf redaction [unclosed: then [font-flags]")).toBe("");
    expect(ruleOf(`[${"x".repeat(65)}] then [font-flags]`)).toBe("");
    expect(ruleOf("pdf redaction [other]: [font-flags]")).toBe("");
    expect(ruleOf("pdf redaction [made-up-thing]: x")).toBe("");
    expect(ruleOf("[IMAGE-IN-REGION]")).toBe("");
    expect(ruleOf(undefined)).toBe("");
    // An inherited property name is not a rule.
    expect(ruleOf("[constructor]")).toBe("");
  });

  it("answers a refusal with its category and rule id, whether Unsupported or Malformed", () => {
    expect(
      messageFor({ kind: "Unsupported", message: "pdf redaction [vector-in-region]: x" }),
    ).toEqual({ ...CATEGORIES.pictures, rule: "vector-in-region", retryable: true });
    expect(messageFor({ kind: "Malformed", message: "pdf region [rotate]: x" })).toEqual({
      ...CATEGORIES["page-shape"],
      rule: "rotate",
      retryable: true,
    });
    expect(messageForRule("acroform-field").title).toBe(CATEGORIES.annotations.title);
    // An unknown rule falls back to the kind's sentence, with no id.
    expect(messageFor({ kind: "Unsupported", message: "[made-up-thing]" }).rule).toBe("");
  });

  it("answers the second reading's refusals with OutputRejected's own copy, never a category (#206)", () => {
    // They arrive as `OutputRejected`, in the shape `redact_read_back.rs`'s `rejected` writes. Their
    // `RULES` entries are placeholders (see the comment there): what a visitor reads is the kind's
    // sentence, with the rule id kept for a report.
    const ids = engineRules().filter((id) => id.startsWith("second-reading-"));
    expect(ids, "the engine's list of second-reading ids").toHaveLength(9);
    const rejected = messageFor({ kind: "OutputRejected", message: "" });
    expect(rejected.title).toBe("Not Only PDF checked the result and would not hand it over.");
    const categoryTitles = new Set(Object.values(CATEGORIES).map((c) => c.title));
    for (const id of ids) {
      const shown = messageFor({
        kind: "OutputRejected",
        message: `redact: the second reading refuses the output -- [${id}] page 1 x`,
      });
      expect(shown.title, id).toBe(rejected.title);
      expect(shown.next, id).toBe(rejected.next);
      expect(shown.rule, id).toBe(id);
      expect(categoryTitles.has(shown.title), `${id} showed a category sentence`).toBe(false);
    }
  });

  it("never puts the engine's message on the page, for any kind (ADR 0009)", () => {
    const CANARY = "SECRET-CANARY-9d1f";
    for (const kind of KINDS) {
      for (const message of [
        `pdf redaction [image-in-region]: ${CANARY}`,
        `pdf redaction [made-up-thing]: ${CANARY}`,
        `${CANARY} [image-in-region]`,
        CANARY,
      ]) {
        const shown = messageFor({ kind, message, limit: "max_pages" });
        for (const [field, value] of Object.entries(shown)) {
          expect(String(value), `${kind}.${field} for ${JSON.stringify(message)}`).not.toContain(
            "CANARY",
          );
        }
      }
    }
  });

  it("names the limit that was hit, and says may, not will", () => {
    expect(
      messageFor({
        kind: "LimitExceeded",
        limit: "max_pages",
        requested: "9000",
        allowed: "2000",
      }).title,
    ).toBe("That document has 9,000 pages, and Not Only PDF stops at 2,000.");
    expect(messageFor({ kind: "LimitExceeded", limit: "max_duration_ms" }).title).toContain(
      "longer than Not Only PDF allows",
    );
    for (const text of everySentence()) expect(text, text).not.toMatch(/\bwill work\b/);
  });

  it("says the §7 result notes only when they happened, with the count in words that agree", () => {
    expect(resultNotes(0, 0)).toEqual([]);
    expect(resultNotes(1, 0)).toHaveLength(1);
    expect(resultNotes(1, 0)[0]).toMatch(/^One font on this page is also used/);
    expect(resultNotes(2, 0)[0]).toMatch(/^2 fonts on this page are also used/);
    expect(resultNotes(0, 1)[0]).toMatch(/^One passage you removed text from had alternative text/);
    expect(resultNotes(4, 3)).toHaveLength(2);
  });
});
