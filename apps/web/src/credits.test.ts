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
import {
  CREDITS_PRESENT_MARK,
  CREDITS_WITHDRAWN_MARK,
  STRIP_WITHDRAWN,
} from "./components/strip-copy.js";

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
 * The component roster the built page actually lists, from its `data-component` attributes.
 *
 * **Not a substring search of the page.** A completeness check written as
 * `html.includes(name)` passes with a component missing: the page is 170 KB of other
 * people's licence text, `"icu"` is a substring of "PARTICULAR", and every BSD disclaimer
 * here contains that word. Measured -- stripping ICU's row and section out of the built HTML
 * left `html.includes("icu")` true. `zlib`, `qpdf`, `pdfium`, `freetype` and
 * `libjpeg-turbo` are all weak the same way, because each appears inside some other
 * component's licence text.
 *
 * `credits.astro` emits the exact name as an attribute for this reason, and says so.
 */
function rosterInPage(): string[] {
  const html = readBuilt(PRODUCTION_DIR, PAGE);
  return [...html.matchAll(/data-component="([^"]*)"/g)].map(([, name]) =>
    name
      .replace(/&#(\d+);/g, (_, n) => String.fromCodePoint(Number(n)))
      .replace(/&quot;/g, '"')
      .replace(/&apos;/g, "'")
      .replace(/&lt;/g, "<")
      .replace(/&gt;/g, ">")
      .replace(/&amp;/g, "&"),
  );
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
    artifacts?: string[];
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

/**
 * The artifacts a WEB reader downloads, and the components distributed with them.
 *
 * DERIVED FROM THE MANIFEST BY THE INDEPENDENT PARSER, not read from `CREDITS.surfaces`.
 * Taking the scope from the generator's own output would make every assertion below a
 * tautology: the page would be asserted to list what the generator decided it should list,
 * and a scope that dropped a component would agree with itself.
 *
 * `artifacts` ("distributed with"), NOT `linked_in` ("code was found in"). A notice
 * obligation attaches to distribution. And `linked_in` is measurably the wrong field here:
 * scoping by it drops `zlib` and `libjpeg-turbo`, both of which
 * `tools/detect-engine-components.py` finds inside the shipped `qpdf.wasm` -- and
 * libjpeg-turbo carries the IJG affirmative notice, so that would be an UNDER-declaration.
 * The manifest's own header says the field "has never been re-derived per artifact".
 */
const WEB_ARTIFACTS = ["qpdf-wasm", "pdfium-wasm"];

/**
 * The artifact a browser fetches only when a page needs a picture of a page (ADR 0026).
 *
 * Stated here, independently of the generator, for the reason the doc comment above gives
 * about `WEB_ARTIFACTS`: taking the split from `CREDITS.surfaces.web.conditional` would make
 * the assertion below agree with whatever the generator decided.
 */
const WEB_CONDITIONAL_ARTIFACTS = ["pdfium-wasm"];

function webComponents() {
  return manifestViaPython().component.filter((c) =>
    (c.artifacts ?? []).some((id) => WEB_ARTIFACTS.includes(id)),
  );
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

  it("credits FreeType again, because the render bundle distributes it — conditionally", () => {
    // THIS TEST WAS THE OTHER WAY ROUND, AND SAID SO. Spike 0004 took PDFium out of the web
    // payload, FreeType reached burrow only through PDFium, and the page correctly stopped
    // crediting it. The assertion carried its own inversion condition verbatim: "if PDFium is
    // back in the payload this test is the wrong way round and the page must credit it again".
    // ADR 0026 is that, so it is inverted rather than deleted — a licence assertion that
    // disappears in the change that makes it matter again is the shape nobody notices.
    //
    // FTL section 2 says "based in part OF the work", which reads like a typo and is not ours
    // to correct; the string is asserted verbatim below for that reason.
    //
    // ASSERTED IN BOTH DIRECTIONS, because this is a licence surface: the component is on the
    // page, AND it is named as one that arrives only with the render bundle. Crediting it
    // without saying which is over-declaration the reader cannot check; saying it without
    // crediting it would be the breach.
    const manifest = manifestViaPython().component;
    const freetype = manifest.find((c) => c.name === "freetype");
    expect(freetype, "freetype left the manifest entirely").toBeDefined();
    expect(freetype?.notice_required, "freetype's notice obligation was dropped").toBeTruthy();
    expect(
      (freetype?.artifacts ?? []).some((id) => WEB_ARTIFACTS.includes(id)),
      "freetype claims no web artifact; if PDFium has left the payload again this test is the " +
        "wrong way round and the page must stop crediting it",
    ).toBe(true);
    expect(
      (freetype?.artifacts ?? []).some(
        (id) => WEB_ARTIFACTS.includes(id) && !WEB_CONDITIONAL_ARTIFACTS.includes(id),
      ),
      "freetype now claims an UNCONDITIONAL web artifact, so the page's 'only if you look at " +
        "page pictures' sentence has become false for it",
    ).toBe(false);

    expect(rosterInPage()).toContain("freetype");
    expect(pageText(), "FTL §2's credit line is not on the page").toContain(
      "based in part of the work of the FreeType Team",
    );
  });

  it("carries the Independent JPEG Group's credit line", () => {
    // IJG LEGAL ISSUES condition (2). Note this one really is "on the work" -- the two
    // obligations differ by one word, which is exactly why both are asserted literally.
    expect(pageText()).toContain("based in part on the work of the Independent JPEG Group");
  });

  it("credits HarfBuzz again, on the same condition and for the same reason", () => {
    // MIT-Modern-Variant requires the notice AND both disclaimer paragraphs. PDFium's package
    // ships no HarfBuzz licence at all, which is why the text is committed at
    // docs/adr/licences/harfbuzz-14.3.1-COPYING.txt (ADR 0010) -- and why losing track of this
    // one would be easy. Same shape as the FreeType case above, inverted by the same change.
    const harfbuzz = manifestViaPython().component.find((c) => c.name.startsWith("harfbuzz"));
    expect(harfbuzz, "harfbuzz left the manifest").toBeDefined();
    expect(harfbuzz?.notice_required, "harfbuzz's notice obligation was dropped").toBeTruthy();
    expect(harfbuzz?.license_text, "harfbuzz's committed licence text was dropped").toBeTruthy();
    expect(
      (harfbuzz?.artifacts ?? []).some((id) => WEB_ARTIFACTS.includes(id)),
      "harfbuzz claims no web artifact",
    ).toBe(true);
    expect(
      (harfbuzz?.artifacts ?? []).some(
        (id) => WEB_ARTIFACTS.includes(id) && !WEB_CONDITIONAL_ARTIFACTS.includes(id),
      ),
      "harfbuzz now claims an UNCONDITIONAL web artifact",
    ).toBe(false);
    expect(rosterInPage().some((n) => n.startsWith("harfbuzz"))).toBe(true);
  });

  it("carries every notice obligation the manifest declares, verbatim", () => {
    // The generalisation of the three above, and the thing that makes them maintainable: a
    // FOURTH obligation cannot be added to the manifest without this page following it.
    //
    // `notice_required` is prose describing the obligation, not the notice text itself, so a
    // substring match on the whole string would be wrong. What must appear verbatim is the
    // licence text the obligation is about -- which is what `license_text` points at.
    const html = pageText();
    const obliged = webComponents().filter((c) => c.notice_required);
    // SCOPED, AND STILL GATED ON A COUNT. One obligation reaches the web today (the IJG line,
    // through libjpeg-turbo inside qpdf.wasm). Zero would mean the scope had swallowed the
    // obligations rather than the artifacts, which is the failure this whole change could
    // plausibly cause and the one nobody would see.
    expect(
      obliged.length,
      "no notice obligation reaches the web surface at all -- the scope has dropped an " +
        "obligation rather than a component",
    ).toBeGreaterThan(0);

    for (const component of obliged) {
      expect(
        component.license_text,
        `${component.name} has an obligation but no text`,
      ).toBeTruthy();
      expect(
        rosterInPage(),
        `${component.name} carries a notice obligation but is not on the page's roster`,
      ).toContain(component.name);
    }
  });

  it("lists every component in the manifest, by name, in its roster", () => {
    // Completeness, from the independent parser, against the page's OWN roster rather than
    // against its prose -- see `rosterInPage`.
    //
    // Note what this does and does not catch. Adding a component to engines/licenses.toml
    // does NOT fail here, and should not: the page is generated from that manifest, so the
    // new component appears on the next build. That is the design working. What fails here
    // is the generation *breaking* -- the hand-rolled TOML reader dropping an entry it cannot
    // parse, the template filtering the roster, `prebuild` not running. Those are the ways a
    // page silently stops covering what we ship, and they are invisible to any assertion
    // written against the generator's own output.
    const expected = webComponents().map((c) => c.name);
    const listed = rosterInPage();

    expect(listed.length, "the page lists no components at all").toBeGreaterThan(0);
    expect(
      listed,
      "the built roster does not match the components distributed with " + WEB_ARTIFACTS.join(", "),
    ).toEqual(expected);
  });

  it("credits NOTHING that the web does not distribute", () => {
    // THE OTHER DIRECTION, and the one the scoping exists for. Without it, a scope that quietly
    // widened back to the whole manifest would pass every assertion above -- the roster would
    // still CONTAIN everything it must contain. Over-declaration is exactly the state this
    // change removed, and it is invisible from a completeness check.
    const shipped = new Set(webComponents().map((c) => c.name));
    const notShipped = manifestViaPython()
      .component.map((c) => c.name)
      .filter((name) => !shipped.has(name));

    expect(
      notShipped.length,
      "every component in the manifest reaches the web, so this test proves nothing -- if " +
        "PDFium is back in the payload, the scope is right and this expectation is stale",
    ).toBeGreaterThan(0);
    for (const name of notShipped) {
      expect(
        rosterInPage(),
        `${name} is credited to a reader who did not download it`,
      ).not.toContain(name);
    }
  });

  it("carries the full licence text of every linked component", () => {
    // `linked` means "confirmed present in a shipped binary by symbol inspection". Those are
    // the ones whose terms actually reach a user's machine.
    const html = pageText();
    const linked = webComponents().filter((c) => c.linked);
    expect(linked.length, "no linked component reaches the web surface").toBeGreaterThan(0);

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

  it("says which components arrive only with the render bundle, and names them", () => {
    // ADR 0026 put PDFium back on the web in a SECOND bundle, so this page now credits more
    // than any one visitor has necessarily downloaded. That is the right direction to be wrong
    // in -- a reader who DOES render is owed FreeType's FTL §2 and HarfBuzz's
    // MIT-Modern-Variant, and a build cannot know in advance whether they will -- but a page
    // that quietly claimed they had downloaded all of it would be a page nobody can check,
    // which is the thing spike 0004 left loose in the other direction.
    //
    // So the page has to SAY it, and it has to say WHICH. Both halves are asserted, because a
    // sentence with no names is the version that survives a component being added.
    const html = pageText();
    const conditional = manifestViaPython().component.filter(
      (c) =>
        (c.artifacts ?? []).some((id) => WEB_CONDITIONAL_ARTIFACTS.includes(id)) &&
        !(c.artifacts ?? []).some(
          (id) => WEB_ARTIFACTS.includes(id) && !WEB_CONDITIONAL_ARTIFACTS.includes(id),
        ),
    );
    expect(
      conditional.length,
      "no component is conditional, so this assertion would pass over nothing",
    ).toBeGreaterThan(0);

    // THE SENTENCE FOLLOWS THE FLAG. Which components a visitor has received depends on
    // whether anything on the site fetches the render bundle, and `STRIP_WITHDRAWN` decides
    // that -- so this assertion takes its words from `strip-copy.ts` rather than pinning the
    // wording of one state. Pinned literally, it failed the moment the strip was withdrawn,
    // for a page that had been correctly updated. `strip-copy.test.ts` owns the agreement in
    // both directions; this one keeps the LICENCE obligation attached to it.
    const said = STRIP_WITHDRAWN ? CREDITS_WITHDRAWN_MARK : CREDITS_PRESENT_MARK;
    expect(
      html.replace(/\s+/g, " "),
      "the page does not say what a visitor has and has not received",
    ).toContain(said);
    for (const component of conditional) {
      expect(
        html,
        `${component.name} arrives only with the render bundle and the page does not say so`,
      ).toContain(component.name);
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
