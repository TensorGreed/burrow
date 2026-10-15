import { readFileSync, readdirSync, statSync } from "node:fs";
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

/** Every `.rs` file under `dir`, recursively. */
function rustFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) out.push(...rustFiles(path));
    else if (path.endsWith(".rs")) out.push(path);
  }
  return out;
}

/**
 * The redaction rule vocabulary, derived from the source rather than typed here.
 *
 * Three shapes carry a rule id, and each is read:
 * - a hyphenated bracketed token anywhere in the engines' and operations' source (`[font-flags]`,
 *   however the message around it is written);
 * - the geometry walk's `Refusal` names, which are stamped into the bracket at run time
 *   (`Self::UnmatchedRestore => "q-without-save"`) and so never appear bracketed in the source;
 * - every rule in `tests/redaction/outcomes.tsv`, the committed record of what real cases refuse.
 *
 * `split-*` rules are split's, never a redaction's, and are left out. The probe below drives it on planted text.
 */
function vocabulary(sources: string[], outcomes: string): Set<string> {
  const ids = new Set<string>();
  for (const text of sources) {
    for (const m of text.matchAll(/\[([a-z][a-z0-9]*(?:-[a-z0-9]+)+)\]/g)) ids.add(m[1] ?? "");
    for (const m of text.matchAll(/Self::\w+ => "([a-z0-9]+(?:-[a-z0-9]+)+)"/g))
      ids.add(m[1] ?? "");
  }
  for (const m of outcomes.matchAll(/\[([a-z0-9-]+)\]/g)) ids.add(m[1] ?? "");
  return new Set([...ids].filter((id) => id !== "" && !id.startsWith("split-")));
}

function realVocabulary(): Set<string> {
  const sources = [
    ...rustFiles(join(REPO, "core/burrow-engines/src")),
    ...rustFiles(join(REPO, "core/burrow-ops/src")),
  ].map((path) => readFileSync(path, "utf8"));
  const outcomes = readFileSync(join(REPO, "tests/redaction/outcomes.tsv"), "utf8");
  return vocabulary(sources, outcomes);
}

/** Bracketed hyphenated words in the source that are not refusal rules. Each says why. */
const NOT_RULES: Readonly<Record<string, string>> = {};

describe("redaction's refusal categories (#136, S3 and S7)", () => {
  it("places every rule the core can raise in a category", () => {
    const found = realVocabulary();
    // A COUNT, so a vocabulary that read nothing does not pass as "nothing unplaced".
    expect(
      found.size,
      "the source scan found too few rules to be reading the core",
    ).toBeGreaterThan(100);
    const unplaced = [...found].filter((id) => !Object.hasOwn(RULES, id) && !(id in NOT_RULES));
    expect(unplaced, `rules with no category: ${unplaced.join(", ")}`).toEqual([]);
  });

  it("places no rule the core does not raise", () => {
    const found = realVocabulary();
    const stale = Object.keys(RULES).filter((id) => !found.has(id));
    expect(stale, `categorised rules the core no longer raises: ${stale.join(", ")}`).toEqual([]);
  });

  it("finds a rule in each of the three shapes, and leaves split's out (the scan's own probe)", () => {
    const found = vocabulary(
      [
        'Err(Error::Unsupported("pdf redaction [planted-bracket]: x".into()))',
        '            Self::Planted => "planted-geometry",',
        '"split [split-planted]: never a redaction rule"',
        "a word [notarule] without a hyphen is not read",
      ],
      'case\tpage\tUnsupported("pdf redaction [planted-outcome]")\n',
    );
    expect([...found].sort()).toEqual(["planted-bracket", "planted-geometry", "planted-outcome"]);
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
    const sentences = [
      ...Object.values(CATEGORIES).flatMap(({ title, next }) => [title, next]),
      ...[
        "PasswordRequired",
        "LimitExceeded",
        "OutputRejected",
        "Malformed",
        "Unsupported",
        "InvalidArgument",
        "EngineUnavailable",
        "Internal",
      ].flatMap((kind) => {
        const { title, next } = messageFor({ kind });
        return [title, next];
      }),
      ...resultNotes(1, 1),
      ...resultNotes(3, 2),
    ];
    for (const text of sentences) {
      expect(text.toLowerCase(), text).not.toContain("burrow");
      for (const banned of ["sorry", "oops", "please", "an error occurred"]) {
        expect(text.toLowerCase(), text).not.toContain(banned);
      }
      const claims = FORBIDDEN_CLAIMS.filter(({ pattern }) => pattern.test(text)).map(
        (c) => c.name,
      );
      expect(claims, text).toEqual([]);
    }
  });

  it("shows a rule id only if it is one of its own", () => {
    expect(ruleOf("pdf redaction [image-in-region]: an image")).toBe("image-in-region");
    // The first KNOWN token wins; an unknown one is skipped, never shown.
    expect(ruleOf("[made-up-thing] then [font-flags]")).toBe("font-flags");
    expect(ruleOf("pdf redaction [made-up-thing]: x")).toBe("");
    expect(ruleOf("[IMAGE-IN-REGION]")).toBe("");
    expect(ruleOf(undefined)).toBe("");
    // An inherited property name is not a rule.
    expect(ruleOf("[constructor]")).toBe("");
  });

  it("answers a refusal with its category, and its rule id", () => {
    const message = messageFor({
      kind: "Unsupported",
      message: "pdf redaction [vector-in-region]: x",
    });
    expect(message).toEqual({ ...CATEGORIES.pictures, rule: "vector-in-region", retryable: true });
    expect(messageForRule("acroform-field").title).toBe(CATEGORIES.annotations.title);
    // An unknown rule falls back to the kind's sentence, with no id.
    expect(messageFor({ kind: "Unsupported", message: "[made-up-thing]" }).rule).toBe("");
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
