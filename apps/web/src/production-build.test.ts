// What a production build actually contains.
//
// A vitest test rather than a Playwright one, deliberately: Playwright serves a build made
// with `BURROW_HARNESS=1`, so it is the wrong process to ask whether a *default* build
// carries the harness. This runs its own build with no flag set and inspects `dist/`.
//
// The claim being checked is not "we intended not to ship the harness". It is "it is not
// there", which is the only version a deploy depends on.

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { HARNESS_DIR, PRODUCTION_DIR as outDir, walk } from "./build-output.js";

// Both builds are produced once by `vitest.global-setup.ts`, not here: three test files now
// read them, and a `beforeAll` per file would mean one Astro build per file.
describe("the production build", () => {
  const files: string[] = walk(outDir);

  it("does not contain the engine harness or the main-thread host", () => {
    const testOnly = files.filter((f) => f.includes("harness") || f.startsWith("host/"));
    expect(testOnly, `these should not ship: ${testOnly.join(", ")}`).toEqual([]);
  });

  it("does not contain the worker state machine, which the harness imports", () => {
    // Named separately from the pattern above because it does not match "harness": the host
    // is two files and only one of them says so. A production build that shipped
    // `host/worker-host.js` alone would have passed the previous assertion while leaving the
    // module the harness driver imports sitting on the origin.
    expect(files, "the state machine is staged for the harness only").not.toContain(
      "host/worker-host.js",
    );
  });

  it("does not expose the harness API anywhere in its output", () => {
    // Not just the route: the identifier must not appear in any shipped file, so a stray
    // import or an inlined chunk cannot reintroduce it.
    const offenders = files.filter((file) => {
      if (!/\.(html|js|mjs|css)$/.test(file)) return false;
      return readFileSync(join(outDir, file), "utf8").includes("burrowHarness");
    });
    expect(offenders, `burrowHarness leaked into: ${offenders.join(", ")}`).toEqual([]);
  });

  it("ships no route to an operation that is held", () => {
    // SPLIT IS HELD. Its outputs can still carry data belonging to pages they excluded —
    // ADR 0019 §2's rule, measured as unmet, tracked as issue #54, and pinned by two
    // deliberately-failing tests in the Rust suite. A tool whose whole claim is that your file
    // does not leave your computer must not offer an operation that can put part of it into a
    // file you then send to somebody else.
    //
    // THE ASSERTION IS ABOUT `dist/`, not about intent. Nobody plans to ship a held tool; what
    // ships is a route, and a route appears the moment somebody adds `src/pages/split-pdf.astro`
    // — which is a one-file change that no other test in this repository would notice. The
    // landing page not linking it is not protection either: an unlinked page is still a page,
    // still indexable, and still reachable by anyone who guesses the URL.
    //
    // This is written as a LIST so lifting the hold is one line, and so the reason travels with
    // the name. An operation comes off it in the pull request that closes the issue holding it.
    const held = [
      { slug: "split-pdf", why: "ADR 0019 §2 / issue #54: outputs can carry excluded pages" },
    ];

    for (const { slug, why } of held) {
      const routes = files.filter(
        (f) => f === `${slug}/index.html` || f === `${slug}.html` || f.startsWith(`${slug}/`),
      );
      expect(routes, `/${slug} must not ship — ${why}`).toEqual([]);
    }

    // AND THE CHECK IS NOT VACUOUS. A filter that matched nothing would pass this whether or
    // not the route existed, so the same patterns are run against a route that DOES ship: if
    // they cannot find `/merge-pdf`, they could not have found `/split-pdf` either.
    const shipped = files.filter(
      (f) => f === "merge-pdf/index.html" || f === "merge-pdf.html" || f.startsWith("merge-pdf/"),
    );
    expect(
      shipped.length,
      "the patterns above cannot find a route that does ship, so they prove nothing about one that must not",
    ).toBeGreaterThan(0);
  });

  it("ships no way to ASK for a held operation, whatever a route is called", () => {
    // THE SLUG IS A NAME; THE OPERATION IS THE THING HELD. The assertion above keys on
    // `/split-pdf`, so `src/pages/extract-pages.astro` posting `op: "split"` would ship a
    // split tool and pass it. Security review made the point: gate on what is actually held.
    //
    // The worker's allowlist is the real control -- `src/worker/main.js` refuses an unknown
    // op before `limits` is even constructed -- and this asserts that the shipped bundle
    // still has the shape that control depends on: the held ops are absent from it, and the
    // ops that ARE allowed are present, so a bundle that stopped containing op names at all
    // could not pass by saying nothing.
    // HELD MEANS HELD FOR A REASON, not merely "not built yet", and the two were conflated
    // here until `reorder` shipped its bridge. `compress` is not written at all, which is not
    // a thing this test can usefully assert about a build: absent code is absent.
    //
    // `split` moved to `allowed` in the pull request that gave it a bridge, which is the third
    // time this list has recorded that lifecycle and the second time for the same reason:
    // the worker can name the operation from that moment, and a list claiming otherwise fails
    // as soon as it becomes true. Its hold was never about the bridge -- ADR 0019 §2's rule was
    // measured as unmet (#54), and #54 closed it. What keeps `/split-pdf` out of the build is
    // the ROUTE assertion above, which is a different check and still holds.
    const held = ["compress"];
    const allowed = [
      "page_count",
      "structure_check",
      "merge",
      "rotate",
      "reorder",
      "split",
      "page_rotations",
    ];

    const scripts = files.filter((f) => /\.(js|mjs)$/.test(f));
    const sources = scripts.map((f) => readFileSync(join(outDir, f), "utf8"));

    for (const op of allowed) {
      const present = sources.some((text) => text.includes(`"${op}"`));
      expect(
        present,
        `no shipped script mentions the op "${op}", so this scan proves nothing`,
      ).toBe(true);
    }
    for (const op of held) {
      const offenders = scripts.filter((f, i) => sources[i].includes(`"${op}"`));
      expect(
        offenders,
        `a shipped script names the held operation "${op}": ${offenders.join(", ")}`,
      ).toEqual([]);
    }
  });

  it("links every tool page it ships, and no tool page it does not", () => {
    // The landing page is where a person finds these. A link to a route that does not exist is
    // a 404 on the one page whose argument is that it tells you the truth about itself; a page
    // that ships with no link is a tool nobody can find. Both have happened here: the first in
    // an early draft, the second to `/merge-pdf`, which shipped without its link.
    const landing = readFileSync(join(outDir, "index.html"), "utf8");
    const linked = [...landing.matchAll(/href="\/([a-z-]+-pdf)"/g)].map((m) => m[1]);
    const routes = files
      .filter((f) => /^[a-z-]+-pdf(\/index)?\.html$/.test(f))
      .map((f) => f.replace(/(\/index)?\.html$/, ""));

    // TWO DIRECTIONS, REPORTED SEPARATELY. One `toEqual` said "links a route that does not
    // ship" for the opposite failure too. And `linked` is de-duplicated: a page legitimately
    // linked twice is not a defect, and comparing raw arrays would have failed on it.
    const linkedSet = [...new Set(linked)].sort();
    const routeSet = [...new Set(routes)].sort();
    expect(
      linkedSet.filter((slug) => !routeSet.includes(slug)),
      "the landing page links a route that does not ship",
    ).toEqual([]);
    expect(
      routeSet.filter((slug) => !linkedSet.includes(slug)),
      "a tool page ships with no link from the landing page, so nobody can find it",
    ).toEqual([]);
    expect(routes.length, "no tool page ships at all, so this compares nothing").toBeGreaterThan(0);
  });

  it("still ships the engines and the worker bundle, which are not test-only", () => {
    // The complement of the assertions above. Without this, deleting too much would pass.
    for (const pattern of [
      /^engines\/pdfium\.[0-9a-f]{16}\.wasm$/,
      /^engines\/qpdf\.[0-9a-f]{16}\.wasm$/,
      /^engines\/burrow_wasm_bg\.[0-9a-f]{16}\.wasm$/,
      /^engines\/burrow-worker\.[0-9a-f]{16}\.js$/,
    ]) {
      expect(
        files.some((f) => pattern.test(f)),
        `nothing matched ${pattern}`,
      ).toBe(true);
    }

    // The credits page is the other thing that is not test-only and must never be swept up
    // by an exclusion. It is a licence obligation (ADR 0008), so its absence is a violation
    // rather than a missing feature -- src/credits.test.ts checks its *contents*; this
    // checks that the exclusion above did not take it with the harness.
    expect(files, "the credits page must ship").toContain("credits/index.html");
  });

  it("ships the worker as ONE bundle, with no glue loose beside it", () => {
    // The Emscripten glue used to be staged as three separate files and pulled in with
    // `importScripts`, which has no integrity mechanism — so 160 KB of third-party PDFium
    // glue ran unverified. It is now inside the worker bundle, covered by that file's
    // digest. A regression would look like these files reappearing.
    // The only `.js` under engines/ may be the worker bundle itself.
    const scripts = files.filter((f) => f.startsWith("engines/") && f.endsWith(".js"));
    expect(scripts, `loose glue beside the bundle: ${scripts.join(", ")}`).toHaveLength(1);
    expect(scripts[0]).toMatch(/^engines\/burrow-worker\.[0-9a-f]{16}\.js$/);

    const source = readFileSync(join(outDir, scripts[0]), "utf8");
    for (const marker of ["createQpdfModule", "FPDF_LoadMemDocument64", "wasm_bindgen"]) {
      expect(source, `the bundle is missing ${marker}`).toContain(marker);
    }
  });

  it("probes its own origin, never a third party", () => {
    // The guard proves a policy is in force by making a request the policy must refuse. If
    // the policy were ever absent, that request actually goes out — so it must be somewhere
    // harmless. A cross-origin probe would make the guard's failure mode the exact thing it
    // guards against: a request to a third party from a page holding a user's file.
    const bundle = files.find((f) => /^engines\/burrow-worker\..*\.js$/.test(f));
    expect(bundle).toBeDefined();
    const source = readFileSync(join(outDir, bundle as string), "utf8");

    const probePath = /const PROBE_PATH = "([^"]+)"/.exec(source)?.[1];
    expect(probePath, "the bundle has no PROBE_PATH").toBeDefined();
    // A path, not a URL: it is resolved against the build's own origin.
    expect(probePath).toMatch(/^\/[A-Za-z0-9_\-/]*$/);
    expect(probePath).not.toMatch(/^https?:/);
    expect(probePath).not.toMatch(/^\/\//);

    // And it must not be a real route, or a missing policy would fetch a live page rather
    // than 404.
    const asRoute = (probePath as string).replace(/^\//, "");
    expect(files).not.toContain(`${asRoute}/index.html`);
    expect(files).not.toContain(`${asRoute}.html`);

    // The origin it is resolved against is this build's own.
    const probeOrigin = /"probeOrigin": "([^"]+)"/.exec(source)?.[1];
    expect(probeOrigin, "the bundle has no probeOrigin").toBeDefined();
    const csp = readFileSync(join(outDir, "_headers"), "utf8");
    expect(csp, "the probe origin is not the origin the policy was built for").toContain(
      probeOrigin as string,
    );

    // The wait for the violation event is bounded, so the guard cannot hang.
    const timeout = /const PROBE_TIMEOUT_MS = (\d+)/.exec(source)?.[1];
    expect(timeout, "the bundle has no PROBE_TIMEOUT_MS").toBeDefined();
    expect(Number(timeout)).toBeGreaterThan(0);
    expect(Number(timeout)).toBeLessThanOrEqual(2_000);
  });

  it("has no misleading strict-mode directive in the worker bundle", () => {
    // The bundle emits a generated const first, so a `"use strict"` anywhere inside it is
    // not in a directive prologue and does nothing. Three of them used to be, which is a
    // comment claiming a guarantee the code does not have.
    //
    // This asserts the decision rather than the mode: either the bundle genuinely starts
    // with the directive, or it contains none of our own. What it must not be is the
    // in-between state where the words are present and inert.
    const bundle = files.find((f) => /^engines\/burrow-worker\..*\.js$/.test(f));
    expect(bundle).toBeDefined();
    const source = readFileSync(join(outDir, bundle as string), "utf8");

    const startsStrict = /^\s*(?:\/\/[^\n]*\n|\s)*"use strict";/.test(source);
    // Emscripten's own glue contains function-level directives, which are real and are not
    // ours. Only top-level ones would be the misleading kind.
    const topLevelDirectives = source
      .split("\n")
      .filter((line) => /^\s{0,2}"use strict";\s*$/.test(line)).length;

    expect(
      startsStrict || topLevelDirectives === 0,
      `the bundle has ${topLevelDirectives} inert top-level "use strict" directive(s)`,
    ).toBe(true);
  });

  it("carries the generated CSP in every page it ships", () => {
    const pages = files.filter((f) => f.endsWith(".html"));
    expect(pages.length).toBeGreaterThan(0);
    for (const page of pages) {
      const html = readFileSync(join(outDir, page), "utf8");
      expect(html, `${page} has no CSP`).toContain('http-equiv="Content-Security-Policy"');
      // Astro emits the attribute double-quoted, so the policy's single quotes are literal.
      expect(html, `${page} CSP is not default-deny`).toContain("default-src 'none'");
    }
  });

  it("writes a _headers file carrying the same policy", () => {
    const headers = join(outDir, "_headers");
    expect(existsSync(headers)).toBe(true);
    const content = readFileSync(headers, "utf8");
    expect(content).toContain("Content-Security-Policy:");
    expect(content).toContain("default-src 'none'");
    // `frame-ancestors` is the reason the header exists as well as the meta tag: browsers
    // ignore it in a <meta> element. `e2e/csp.spec.ts` says this test asserts it, so it had
    // better.
    expect(content, "frame-ancestors only works in a header").toContain("frame-ancestors 'none'");
  });

  it("has the harness when it is asked for, so the exclusion is doing the work", () => {
    // The control. Without it, an integration that deleted the route unconditionally --- or
    // a build that never produced it --- would pass every assertion above.
    const built = walk(HARNESS_DIR);
    expect(built.some((f) => f.includes("harness"))).toBe(true);
    // Both halves of the control: the page AND the host it loads. Asserting only the page
    // would let an exclusion that deleted `host/` unconditionally pass, which would leave the
    // harness route present and broken rather than absent.
    expect(built, "the driver must be there when the harness is").toContain(
      "host/harness-driver.js",
    );
    expect(built).toContain("host/worker-host.js");
  });

  it("ships no inline <style>, because the CSP would refuse it", () => {
    // `style-src 'self'` carries no `'unsafe-inline'` and no nonce (ADR 0014), so an
    // inline <style> is refused by the browser and the page renders unstyled. Nothing in
    // the source asks for one — it would arrive as a side effect of Astro's
    // `build.inlineStylesheets: "auto"`, which reads `vite.build.assetsInlineLimit`
    // (0 in astro.config.mjs) and inlines any stylesheet under it. Raise that number for a
    // payload reason and the whole site loses its CSS. Found by security review of M1 PR A.
    const pages = walk(outDir).filter((f) => f.endsWith(".html"));
    expect(pages.length, "no HTML in the build, so this would check nothing").toBeGreaterThan(0);

    const withInlineStyle = pages.filter((f) =>
      readFileSync(join(outDir, f), "utf8").includes("<style"),
    );
    expect(withInlineStyle).toEqual([]);

    // The near-miss: these pages do carry stylesheets, as external links. Without this, a
    // build that shipped no CSS at all would satisfy the assertion above.
    const withLinkedStyle = pages.filter((f) =>
      /<link[^>]+rel="stylesheet"/.test(readFileSync(join(outDir, f), "utf8")),
    );
    expect(withLinkedStyle).toEqual(pages);
  });
});
