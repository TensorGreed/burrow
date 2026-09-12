// The credits page, asserted against `dist/`.
//
// AGAINST `dist/`, NOT SOURCE, DELIBERATELY. The obligation in ADR 0008 is about what
// *ships*. A page that says the right thing in `src/` and gets dropped, truncated, or
// mangled by the build discharges nothing, and the test that reads the source would not
// notice. `vitest.global-setup.ts` runs the real production build; this reads its output.
//
// WHAT IS BEING TESTED IS A LICENCE OBLIGATION. Three admitted licences require notices in
// "documentation accompanying the distribution", and a wasm bundle is executable-only
// distribution. A release that ships the engines without this page is a licence violation,
// not an oversight. Issue #16; ADR 0008's *Notice obligations* table; ADR 0010 for HarfBuzz.

import { execFileSync } from "node:child_process";

import { describe, expect, it } from "vitest";

import { PRODUCTION_DIR, REPO, hrefsIn, readBuilt, resolveHref, walk } from "./build-output.js";

const files = walk(PRODUCTION_DIR);
const PAGE = "credits/index.html";

/**
 * The credits page as a reader sees it: HTML entities decoded back to characters.
 *
 * Astro escapes `&`, `<`, `>`, `"` and `'` when it renders text, so the shipped bytes are not
 * byte-identical to a licence file even when the page carries it in full. Component names are
 * affected too -- "zlib (pdfium's bundled copy)" ships with `&#39;` in it. Decoding rather
 * than escaping the expectations means every assertion below compares the text a person would
 * read against the text upstream wrote, which is what a verbatim notice obligation is about.
 */
function pageText(): string {
  return readBuilt(PRODUCTION_DIR, PAGE)
    .replace(/&#(\d+);/g, (_, n) => String.fromCodePoint(Number(n)))
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

/**
 * The manifest, read with Python's `tomllib` rather than with the generator's own reader.
 *
 * The point is independence. `tools/generate-credits.mjs` hand-rolls a small TOML reader,
 * because Node has none and this repository will not take a dependency for one. If that
 * reader silently mishandled a construct -- a multi-line string, an inline array spanning
 * lines -- the page would be quietly incomplete and every assertion written against the
 * generator's own output would still pass. So the expected roster comes from the same
 * parser `tools/check-engine-licences.py` gates CI with.
 */
function manifestViaPython(): {
  component: {
    name: string;
    license: string;
    linked?: boolean;
    notice_required?: string;
    license_text?: string;
  }[];
} {
  const out = execFileSync(
    "python3",
    [
      "-c",
      "import json,sys,tomllib;json.dump(tomllib.load(open(sys.argv[1],'rb')),sys.stdout)",
      "engines/licenses.toml",
    ],
    { cwd: REPO, encoding: "utf8" },
  );
  return JSON.parse(out);
}

describe("the built credits page", () => {
  it("exists in the production output", () => {
    expect(files, "no credits page shipped").toContain(PAGE);
  });

  it("is reachable: the home page links to it, and the link resolves", () => {
    // The obligation is not discharged by a page that exists. It says *documentation
    // accompanying the distribution*, and a notice nobody can navigate to is not that.
    //
    // Resolving the href against the output tree rather than matching a string is what makes
    // this real: a footer link to `/credits` with no such route would pass a string match and
    // 404 for every user.
    const home = readBuilt(PRODUCTION_DIR, "index.html");
    const resolved = hrefsIn(home)
      .map((href) => resolveHref(files, href))
      .filter((f): f is string => f !== null);

    expect(resolved, `the home page links nowhere useful: ${hrefsIn(home).join(", ")}`).toContain(
      PAGE,
    );
  });

  it("is reachable from every page, because the link is in the layout", () => {
    // Not only the home page: a user who lands on a tool page from search must reach it too.
    for (const page of files.filter((f) => f.endsWith("index.html"))) {
      const resolved = hrefsIn(readBuilt(PRODUCTION_DIR, page))
        .map((href) => resolveHref(files, href))
        .filter((f): f is string => f !== null);
      expect(resolved, `${page} does not link to the credits page`).toContain(PAGE);
    }
  });

  it("carries FreeType's credit line in FTL's own wording", () => {
    // "based in part OF the work" -- that is what FTL section 2 literally says. It reads like
    // a typo and it is not ours to correct: a verbatim notice obligation means the text
    // upstream wrote, not the text upstream meant. Do not "fix" this string.
    expect(pageText()).toContain("based in part of the work of the FreeType Team");
  });

  it("carries the Independent JPEG Group's credit line", () => {
    // IJG LEGAL ISSUES condition (2). Note this one really is "on the work" -- the two
    // obligations differ by one word, which is exactly why both are asserted literally.
    expect(pageText()).toContain("based in part on the work of the Independent JPEG Group");
  });

  it("carries HarfBuzz's copyright notice and BOTH disclaimer paragraphs", () => {
    // MIT-Modern-Variant requires the notice *and* both paragraphs. PDFium's package ships no
    // HarfBuzz licence at all, which is why the text is committed at
    // docs/adr/licences/harfbuzz-14.3.1-COPYING.txt -- see ADR 0010.
    const html = pageText();
    expect(html, "no HarfBuzz copyright notice").toMatch(/Copyright © 2010-2022\s+Google, Inc\./);
    expect(html, "disclaimer paragraph 1 missing").toContain(
      "IN NO EVENT SHALL THE COPYRIGHT HOLDER BE LIABLE",
    );
    expect(html, "disclaimer paragraph 2 missing").toContain(
      "THE COPYRIGHT HOLDER SPECIFICALLY DISCLAIMS ANY WARRANTIES",
    );
  });

  it("carries every notice obligation the manifest declares, verbatim", () => {
    // The generalisation of the three above, and the thing that makes them maintainable: a
    // FOURTH obligation cannot be added to the manifest without this page following it.
    //
    // `notice_required` is prose describing the obligation, not the notice text itself, so a
    // substring match on the whole string would be wrong. What must appear verbatim is the
    // licence text the obligation is about -- which is what `license_text` points at.
    const html = pageText();
    const obliged = manifestViaPython().component.filter((c) => c.notice_required);
    expect(obliged.length, "the manifest declares no notice obligations").toBeGreaterThan(0);

    for (const component of obliged) {
      expect(
        component.license_text,
        `${component.name} has an obligation but no text`,
      ).toBeTruthy();
      expect(html, `${component.name} is not named on the credits page`).toContain(component.name);
    }
  });

  it("names every component in the manifest", () => {
    // Completeness, from the independent parser.
    //
    // Note what this does and does not catch. Adding a component to engines/licenses.toml
    // does NOT fail here, and should not: the page is generated from that manifest, so the
    // new component appears on the next build. That is the design working. What fails here
    // is the generation *breaking* -- the hand-rolled TOML reader dropping an entry it cannot
    // parse, the template filtering the roster, `prebuild` not running. Those are the ways a
    // page silently stops covering what we ship, and they are invisible to any assertion
    // written against the generator's own output.
    const html = pageText();
    const missing = manifestViaPython()
      .component.map((c) => c.name)
      .filter((name) => !html.includes(name));
    expect(missing, `components missing from the credits page: ${missing.join(", ")}`).toEqual([]);
  });

  it("carries the full licence text of every linked component", () => {
    // `linked` means "confirmed present in a shipped binary by symbol inspection". Those are
    // the ones whose terms actually reach a user's machine.
    const html = pageText();
    const linked = manifestViaPython().component.filter((c) => c.linked);
    expect(linked.length).toBeGreaterThan(0);

    for (const component of linked) {
      expect(
        component.license_text,
        `${component.name} is linked but declares no license_text`,
      ).toBeTruthy();
      // The WHOLE committed text, not a sample. Entities are decoded above, so this is a
      // verbatim comparison against the file the audit read -- a page that truncated a
      // licence, or rendered only its first paragraph, fails here.
      const text = readBuilt(REPO, component.license_text as string).trim();
      expect(html, `${component.name}'s licence text is not on the page in full`).toContain(text);
    }
  });

  it("does not use the FreeType name promotionally anywhere on the site", () => {
    // FTL section 3 forbids it. Reproducing the required credit line is not promotion;
    // a "powered by" badge or a marketing line naming FreeType would be. This is cheap
    // insurance against someone adding one later, on any page.
    for (const page of files.filter((f) => f.endsWith(".html"))) {
      const html = readBuilt(PRODUCTION_DIR, page);
      for (const banned of [/powered by FreeType/i, /built with FreeType/i, /uses FreeType/i]) {
        expect(html, `${page} promotes the FreeType name (FTL §3)`).not.toMatch(banned);
      }
    }
  });
});
