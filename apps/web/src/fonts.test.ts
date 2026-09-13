// The shipped typeface is the bytes `apps/web/fonts.toml` says it is.
//
// WHY A DIGEST AND NOT A SHAPE CHECK
//
// A binary asset in `public/` is unreviewable: nobody reads 18 KB of woff2 in a pull
// request. The manifest is the review surface — where it came from, pinned to an upstream
// commit, what was done to it, and by which version of which tool — and this test is what
// stops the manifest from describing a file other than the one that ships.
//
// The digest is deliberately the whole of the check. Parsing the font here to re-derive
// its glyph coverage or its name records would be strictly weaker: the digest already
// fixes every byte, so a second, looser assertion over the same bytes could only add a way
// to pass while the digest failed. What the font contains was verified with fontTools at
// vendoring time, against THESE bytes, and the manifest records the findings; this test
// keeps those findings attached to the file they were made about.
//
// A TRIPWIRE, NOT A CHECK. This file joins `font.file` and `font.licence_file` to REPO and
// reads them with no containment check, unlike `tools/generate-credits.mjs`'s
// `resolveLicenceText`, which refuses any path outside the committed licence directories.
// The asymmetry is deliberate and defensible for the reason that function itself gives:
// its output is embedded verbatim into a PUBLISHED page, while nothing here has a sink —
// the bytes are hashed and compared as hex, and a mismatch prints two digests, never
// content. The worst a hostile committed manifest achieves is a failing test.
//
// THE MOMENT ANY FIELD FROM fonts.toml REACHES A BUILD OUTPUT, A PAGE, OR AN ERROR
// MESSAGE, IT NEEDS THAT CONTAINMENT CHECK. Rendering the font manifest onto /credits is
// the obvious next thing someone will want; that is the change that makes this a hole.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { parseToml } from "../../../tools/toml.mjs";
import { REPO } from "./build-output.js";

const MANIFEST_PATH = join(REPO, "apps", "web", "fonts.toml");
const manifest = parseToml(readFileSync(MANIFEST_PATH, "utf8"), "apps/web/fonts.toml") as {
  font?: Record<string, string | number>[];
};

const fonts = manifest.font ?? [];

const sha256 = (bytes: Buffer) => createHash("sha256").update(bytes).digest("hex");

/**
 * Does an OFL copyright line declare a Reserved Font Name?
 *
 * A named function rather than a regex inline in an assertion, so the probe below can call
 * the same rule the per-font check calls. See that probe for why.
 */
export function declaresReservedFontName(copyright: string): boolean {
  return /reserved font name/i.test(copyright);
}

describe("fonts.toml", () => {
  it("declares at least one font", () => {
    // Without this, every per-font assertion below iterates an empty list and the suite
    // reports success having examined nothing — the failure mode CLAUDE.md names.
    expect(
      fonts.length,
      "no [[font]] entries, so every assertion below would be vacuous",
    ).toBeGreaterThan(0);
  });

  it("names every field a reader needs to reproduce the asset", () => {
    const required = [
      "id",
      "family",
      "version",
      "licence",
      "licence_file",
      "licence_file_sha256",
      "copyright",
      "source_repo",
      "source_path",
      "source_commit",
      "source_sha256",
      "file",
      "sha256",
      "bytes",
      "build_tool",
      "build_commands",
    ];
    for (const font of fonts) {
      const missing = required.filter((key) => font[key] === undefined || font[key] === "");
      expect(missing, `${String(font.id)}: fonts.toml is missing ${missing.join(", ")}`).toEqual(
        [],
      );
    }
  });

  it("pins the source commit to a full hash, not a branch or a short sha", () => {
    // A branch name, or a 7-character prefix, is not a pin: the first moves and the second
    // can collide. engines/pins.toml holds the same line for the engines.
    for (const font of fonts) {
      expect(String(font.source_commit), `${String(font.id)}: source_commit`).toMatch(
        /^[0-9a-f]{40}$/,
      );
    }
  });

  it("pins the build tool to an exact version", () => {
    // Subsetters change their output between releases. Without a version, "re-run this and
    // compare the digest" is not a check anyone can perform.
    for (const font of fonts) {
      expect(String(font.build_tool), `${String(font.id)}: build_tool`).toMatch(/\d+\.\d+\.\d+/);
    }
  });
});

describe("the shipped font files", () => {
  it.each(fonts.map((font) => [String(font.id), font]))(
    "%s matches its recorded digest and size",
    (_id, font) => {
      const bytes = readFileSync(join(REPO, String(font.file)));
      expect(sha256(bytes)).toBe(String(font.sha256));
      expect(bytes.length).toBe(Number(font.bytes));
    },
  );

  it.each(fonts.map((font) => [String(font.id), font]))(
    "%s ships its licence text, unmodified",
    (_id, font) => {
      // OFL 1.1 section 1 requires the copyright notice and the licence to travel with the
      // Font Software. The name table carries both inside the file; this is the copy a
      // person can read. Digested so an edit to it is a failing test rather than a quiet
      // divergence from the upstream text.
      const licence = readFileSync(join(REPO, String(font.licence_file)));
      expect(sha256(licence)).toBe(String(font.licence_file_sha256));
    },
  );

  it.each(fonts.map((font) => [String(font.id), font]))(
    "%s declares a licence that is on the ADR 0008 allowlist",
    (_id, font) => {
      // Fonts are admitted as "OFL (fonts)". A font under anything else needs an ADR, not a
      // row in fonts.toml — the same rule engines/licenses.toml states for components.
      expect(String(font.licence)).toBe("OFL-1.1");
    },
  );

  it.each(fonts.map((font) => [String(font.id), font]))(
    "%s carries no Reserved Font Name, so subsetting it may keep the family name",
    (_id, font) => {
      // OFL section 3 forbids a Modified Version from using a Reserved Font Name, and
      // subsetting IS modification. The convention is to declare one in the copyright line
      // as `... with Reserved Font Name "Foo"`. If upstream ever adds one, this fails and
      // the font must be renamed or dropped — which is the point: it is a one-line change
      // upstream that would otherwise go unnoticed through a version bump.
      expect(declaresReservedFontName(String(font.copyright))).toBe(false);
    },
  );

  it("recognises a copyright line that does declare a Reserved Font Name", () => {
    // THE PROBE MUST CALL THE RULE, NOT A COPY OF IT. An earlier version of this test
    // inlined the regex in the assertion above and matched a second, identical regex
    // against a literal here. Code review mutated the real one to
    // `/zzz reserved font name zzz/i` and all nine tests stayed green — the rule matched
    // nothing and the probe was checking itself. That is the exact failure
    // `tokens-check.ts`'s header describes, reproduced in the file written to avoid it.
    expect(
      declaresReservedFontName('Copyright 2020 The Foo Authors with Reserved Font Name "Foo"'),
    ).toBe(true);
    expect(declaresReservedFontName("Copyright 2020 The Foo Authors")).toBe(false);
  });
});
