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

  it("ships every tool page whose operation is not held", () => {
    // SPLIT'S HOLD IS LIFTED, AND THIS IS THE SAME CHECK POINTING THE OTHER WAY.
    //
    // It read `const held = [{ slug: "split-pdf", why: "ADR 0019 §2 / issue #54: outputs can
    // carry excluded pages" }]` and asserted the route was ABSENT, because a tool whose whole
    // claim is that your file does not leave your computer must not offer an operation that
    // puts part of it into a file you then send to somebody else. #54 closed that -- the
    // pruning policy takes back out what the engine's reachability closure drags in,
    // `subset_closure.rs` asserts the structural property over every object in the source,
    // and each part verifies its own output before it is posted (ADR 0022).
    //
    // The list is kept rather than deleted. `compress` is not written at all, which is not a
    // thing this assertion can say anything about -- absent code is absent -- so `held` is
    // empty and the SHIPPED list is what carries the weight now. A route that must not ship
    // goes back on `held`, and the loop below is waiting for it.
    const held: { slug: string; why: string }[] = [];
    const shipped = ["merge-pdf", "split-pdf", "rotate-pdf", "reorder-pdf", "compress-pdf"];

    const routesFor = (slug: string) =>
      files.filter(
        (f) => f === `${slug}/index.html` || f === `${slug}.html` || f.startsWith(`${slug}/`),
      );

    for (const { slug, why } of held) {
      expect(routesFor(slug), `/${slug} must not ship — ${why}`).toEqual([]);
    }

    // GATED ON THE COUNT, not merely on each one being found: a slug dropped from this list
    // would take its assertion with it and the suite would still pass.
    expect(shipped).toHaveLength(5);
    for (const slug of shipped) {
      expect(routesFor(slug), `/${slug} must ship`).not.toEqual([]);
    }
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
    // measured as unmet (#54), and #54 closed it. The ROUTE assertion above now asserts
    // `/split-pdf` SHIPS, so this list and that one agree about split for the first time.
    // FOURTH TIME, AND COMPRESS'S TURN. Phase 3 gave `compress` a bridge -- an
    // `impl DocumentCompressor for WebQpdf`, a wasm entry point and a worker dispatch branch
    // -- so from that moment the worker can name the operation, and a list claiming otherwise
    // fails as soon as it becomes true. That is the lifecycle this comment describes working,
    // not an exception to it.
    //
    // **It is NOT held for a reason and never was.** It was on this list only because it was
    // not written, which the paragraph above says is exactly the conflation to avoid. So it
    // moves to `allowed` rather than staying with a new justification.
    //
    // `/compress-pdf` now ships too, so the route list above asserts it, and this list is
    // EMPTY for the first time. An empty held list gates on nothing, so the loop below would
    // pass over an empty set -- which is why the `allowed` loop exists and is the half that
    // carries the measurement: each of the eight names must be found in a shipped script, so
    // a scan that stopped finding anything fails rather than reporting no offenders.
    const held: string[] = [];
    const allowed = [
      "page_count",
      "structure_check",
      "merge",
      "rotate",
      "reorder",
      "split",
      "compress",
      "page_rotations",
    ];

    // GATED ON ITS OWN LENGTH, because the loop below is now the only half that measures
    // anything: with `held` empty, a name quietly dropped from `allowed` would take its
    // assertion with it and the scan would report no offenders over a shorter list.
    expect(allowed).toHaveLength(8);

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
    // `pdfium.wasm` was required here until spike 0004 took it out of the payload. The
    // absence is not asserted in this file: `tools/check-pdfium-is-render-only.sh` does that,
    // with a positive control and the CSP, which is more than a filename pattern can say.
    for (const pattern of [
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

  it("ships each worker as ONE bundle, with no glue loose beside them", () => {
    // The Emscripten glue used to be staged as separate files and pulled in with
    // `importScripts`, which has no integrity mechanism — so third-party glue ran unverified.
    // It is now inside a worker bundle, covered by that file's digest. A regression would
    // look like those files reappearing.
    //
    // TWO BUNDLES SINCE ADR 0026, AND THE COUNT IS EXACT RATHER THAN AN UPPER BOUND. Each is
    // one file the page fetches with `integrity` and wraps in a Blob, so "one digest covers
    // every line of worker code" holds per bundle. A third `.js` under `engines/` is either
    // loose glue or a bundle nobody declared, and both are worth failing on.
    const scripts = files.filter((f) => f.startsWith("engines/") && f.endsWith(".js")).sort();
    expect(scripts, `loose glue beside the bundles: ${scripts.join(", ")}`).toHaveLength(2);

    // Named, not counted. Two files of the right shape could still be the wrong two.
    const expected = [
      {
        pattern: /^engines\/burrow-render-worker\.[0-9a-f]{16}\.js$/,
        // The render bundle's proof that the BRIDGE is in it and not only the glue, and that
        // it is the PDFium one. `_FPDF_LoadMemDocument64` is the glue's export; the bridge
        // global is ours.
        markers: ["self.__burrow_pdfium_", "_FPDF_GetPageCount(", "wasm_bindgen"],
        // AND WHAT MUST NOT BE THERE. The render bundle does not link qpdf: its Rust module
        // is built `--no-default-features --features render`, so a qpdf bridge global here
        // would mean the wrong artifact was staged.
        //
        // NEEDLES THAT ONLY OCCUR AS CODE, because `prelude.js` and `worker-protocol.js` are
        // shared by both bundles and their comments legitimately discuss both engines — a
        // bare `createQpdfModule` matches the sentence in `render-main.js` explaining what
        // the OTHER bundle does. `tools/check-pdfium-is-render-only.sh` can stay exact in the
        // other direction because no shared file spells `FPDF_` or `__burrow_pdfium_` in
        // prose; here it cannot, and picking a call shape is better than teaching a scanner
        // to strip comments out of 240 KB of third-party glue.
        absent: ["self.__burrow_qpdf_", "_qpdf_read_memory("],
      },
      {
        pattern: /^engines\/burrow-worker\.[0-9a-f]{16}\.js$/,
        markers: ["createQpdfModule(", "self.__burrow_qpdf_", "wasm_bindgen"],
        // THE CLAIM THE BASE PAYLOAD MAKES: a person who merges two files downloads no
        // PDFium. `tools/check-pdfium-is-render-only.sh` is the full check over the whole
        // build; this is the same property asserted where the bundle is assembled, because
        // the bundle is the thing whose source list decides it.
        absent: ["__burrow_pdfium_", "FPDF_"],
      },
    ];

    for (const { pattern, markers, absent } of expected) {
      const name = scripts.find((f) => pattern.test(f));
      expect(name, `no bundle matched ${pattern}`).toBeDefined();
      const source = readFileSync(join(outDir, name as string), "utf8");
      for (const marker of markers) {
        expect(source, `${name} is missing ${marker}`).toContain(marker);
      }
      for (const marker of absent) {
        expect(
          source,
          `${name} contains ${marker}, which belongs to the other bundle`,
        ).not.toContain(marker);
      }
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
