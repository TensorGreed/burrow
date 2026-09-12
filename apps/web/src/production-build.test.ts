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
});
