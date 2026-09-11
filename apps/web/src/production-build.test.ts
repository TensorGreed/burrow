// What a production build actually contains.
//
// A vitest test rather than a Playwright one, deliberately: Playwright serves a build made
// with `BURROW_HARNESS=1`, so it is the wrong process to ask whether a *default* build
// carries the harness. This runs its own build with no flag set and inspects `dist/`.
//
// The claim being checked is not "we intended not to ship the harness". It is "it is not
// there", which is the only version a deploy depends on.

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { beforeAll, describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const webApp = resolve(here, "..");
const outDir = join(webApp, "dist-production-check");

/** Every file under `dir`, as paths relative to it. */
function walk(dir: string, prefix = ""): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const full = join(dir, entry);
    const rel = prefix ? `${prefix}/${entry}` : entry;
    return statSync(full).isDirectory() ? walk(full, rel) : [rel];
  });
}

describe("the production build", () => {
  let files: string[] = [];

  beforeAll(() => {
    rmSync(outDir, { recursive: true, force: true });
    // No BURROW_HARNESS in the environment: this is what a deploy runs.
    const env = { ...process.env };
    delete env.BURROW_HARNESS;
    execFileSync("node", [resolve(webApp, "../../tools/stage-web-engines.mjs")], {
      cwd: webApp,
      env,
      stdio: "pipe",
    });
    execFileSync("pnpm", ["exec", "astro", "build", "--outDir", outDir], {
      cwd: webApp,
      env,
      stdio: "pipe",
    });
    files = walk(outDir);
  }, 300_000);

  it("does not contain the engine harness or its probe worker", () => {
    const testOnly = files.filter((f) => f.includes("harness") || f.includes("csp-probe"));
    expect(testOnly, `these should not ship: ${testOnly.join(", ")}`).toEqual([]);
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
  });

  it("has the harness when it is asked for, so the exclusion is doing the work", () => {
    // The control. Without it, an integration that deleted the route unconditionally --- or
    // a build that never produced it --- would pass every assertion above.
    const withHarness = join(webApp, "dist-harness-check");
    rmSync(withHarness, { recursive: true, force: true });
    execFileSync("pnpm", ["exec", "astro", "build", "--outDir", withHarness], {
      cwd: webApp,
      env: { ...process.env, BURROW_HARNESS: "1" },
      stdio: "pipe",
    });
    const built = walk(withHarness);
    expect(built.some((f) => f.includes("harness"))).toBe(true);
    rmSync(withHarness, { recursive: true, force: true });
  }, 300_000);
});
