// Every kind the binding can produce has a sentence on every page that can receive it —
// checked against the Rust source rather than against a hand-written list.
//
// # Why this file exists
//
// `merge-messages.test.ts`, `rotate-messages.test.ts` and `reorder-messages.test.ts` each
// carry a `KINDS` array described as mirroring `kind_of`. All three went stale the day
// `OutputRejected` was added in Rust, and the test that claims to police them — "names every
// kind the binding can produce" — only asserts the list has no duplicates and contains a
// known entry, so it cannot see a kind that was never added. That is `CLAUDE.md`'s "a check
// that silently examines nothing reads as coverage", and the consequence was real: on the
// exact document ADR 0022 exists for, the page said "Your file is fine … Try again", and
// both halves were false.
//
// So the list is derived. Add a kind in Rust and give no page a sentence for it, and this
// fails naming the kind.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { messageFor as mergeMessage } from "./merge-messages.js";
import { messageFor as reorderMessage } from "./reorder-messages.js";
import { messageFor as splitMessage } from "./split-messages.js";
import { messageFor as rotateMessage } from "./rotate-messages.js";

/** Every string `kind_of` can return, read out of the binding itself. */
function kindsFromRust(): string[] {
  const source = readFileSync(
    new URL("../../../../bindings/burrow-wasm/src/lib.rs", import.meta.url),
    "utf8",
  );
  const start = source.indexOf("fn kind_of(");
  expect(
    start,
    "kind_of has moved or been renamed in bindings/burrow-wasm/src/lib.rs",
  ).toBeGreaterThan(-1);
  const body = source.slice(start, source.indexOf("\n}\n", start));

  // Only the right-hand side of a match arm, so a kind named in a comment is not counted.
  const kinds = [...body.matchAll(/=>\s*"([A-Za-z]+)"/g)].map((m) => m[1]);
  expect(kinds.length, "no match arms found; the parse is wrong, not the source").toBeGreaterThan(
    5,
  );
  return [...new Set(kinds)];
}

/**
 * The fallback each page gives a kind it does not name.
 *
 * "Something inside burrow failed / Your file is fine … Try again" — correct for `Io` and
 * `Internal`, and wrong for anything else, which is what makes comparing against it a test.
 */
const UNHANDLED = "__not_a_kind_any_binding_produces__";

/**
 * Kinds a page may legitimately leave to the fallback, with the reason.
 *
 * Named rather than inferred: an exclusion nobody had to write down is an exclusion nobody
 * reviewed.
 */
const FALLBACK_IS_CORRECT: Record<string, string[]> = {
  // The fallback IS the message for these two.
  Io: ["merge", "rotate", "reorder", "split"],
  Internal: ["merge", "rotate", "reorder", "split"],
  // `Error` is `#[non_exhaustive]`; "Unknown" is the conservative arm and has no sentence of
  // its own by design.
  Unknown: ["merge", "rotate", "reorder", "split"],
  // One input, so no input can be named. `merge` must handle it and does -- through the
  // INNER kind, which is why the probe below hands it one.
  InputFailed: ["rotate", "reorder", "split"],
};

type Probe = { kind: string; innerKind?: string };

const PAGES = [
  { name: "merge", messageFor: mergeMessage as (f: Probe) => { title: string } },
  { name: "rotate", messageFor: rotateMessage as (f: Probe) => { title: string } },
  { name: "reorder", messageFor: reorderMessage as (f: Probe) => { title: string } },
  { name: "split", messageFor: splitMessage as (f: Probe) => { title: string } },
];

/**
 * What to hand `messageFor` for a kind.
 *
 * `InputFailed` is a WRAPPER: `merge-messages.ts` reads the inner kind and reports that, so
 * probing it bare is probing a case the binding never sends -- the wrapper always carries
 * one. Getting this wrong made the check fail against correct code, which is its own kind of
 * dishonest test.
 */
function probe(kind: string): Probe {
  return kind === "InputFailed" ? { kind, innerKind: "Malformed" } : { kind };
}

describe("the binding's error kinds", () => {
  it("are all handled, on every page that can receive them", () => {
    const kinds = kindsFromRust();

    // REPORTED, not merely gated: a parse that found three kinds would otherwise pass three
    // assertions and read as success.
    expect(kinds.sort()).toEqual(
      [
        "InputFailed",
        "Internal",
        "InvalidArgument",
        "Io",
        "LimitExceeded",
        "Malformed",
        "OutputRejected",
        "PasswordRequired",
        "Unknown",
        "Unsupported",
      ].sort(),
    );

    // NOT FROM `kind_of`. The host invents this one when the circuit breaker has latched
    // (ADR 0015 §3) and no Rust error was involved, so it is asserted beside the derived list
    // rather than expected to appear in it -- and it is asserted, because a page without a
    // sentence for it offers a control that is not on the page.
    const HOST_KINDS = ["EngineUnavailable"];

    for (const page of PAGES) {
      const fallback = page.messageFor({ kind: UNHANDLED }).title;
      for (const kind of [...kinds, ...HOST_KINDS]) {
        const allowed = FALLBACK_IS_CORRECT[kind]?.includes(page.name) ?? false;
        const title = page.messageFor(probe(kind)).title;
        if (allowed) continue;
        expect(
          title,
          `${page.name} has no sentence for ${kind}; it falls to the generic one`,
        ).not.toBe(fallback);
      }
    }
  });

  it("would notice a kind that lost its sentence", () => {
    // THE CONTROL. Every assertion above is satisfied by a comparison that can never fail,
    // which is what a fallback-vs-fallback comparison would be. `Io` is handled BY the
    // fallback, so it must compare equal to it.
    for (const page of PAGES) {
      expect(page.messageFor({ kind: "Io" }).title).toBe(
        page.messageFor({ kind: UNHANDLED }).title,
      );
    }
  });
});
