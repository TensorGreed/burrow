import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { PRODUCTION_DIR } from "./build-output.js";

/**
 * The origin reaches every place that needs it, and they all say the same thing.
 *
 * SIX PLACES, ONE INPUT. `BURROW_SITE` is read once, in `tools/build-origin.mjs`, and comes
 * out in:
 *
 *   1. `connect-src` in the generated CSP — the `<meta>` in every page and `_headers`;
 *   2. the worker bundle's absolute engine URLs;
 *   3. `<link rel="canonical">`, via Astro's `site:`;
 *   4. `<meta name="burrow-built-for">`, which `src/origin-guard.ts` compares at run time;
 *   5. `sitemap.xml`'s `<loc>` entries;
 *   6. `robots.txt`'s `Sitemap:` line.
 *
 * FIVE AND SIX ARRIVED WITH THE CUSTOM DOMAIN, and they are the two where being wrong is
 * quietest: nothing on the page breaks, and the failure is that a search engine is asked to
 * index an origin this build is not for. The same `dist/` is served on `burrow-f2s.pages.dev`
 * as well, permanently and with no redirect available, so these files and the canonical link
 * are the whole of what stops one site's traffic being counted as two.
 *
 * They agree by construction now. This test exists because they did NOT: `astro.config.mjs`
 * set no `site:` at all, so every shipped page carried
 * `<link rel="canonical" href="http://localhost/...">` while the CSP named the real origin.
 * Two independent places to configure one thing, one of them silently wrong — and invisible
 * in every local test, because localhost is where local tests run.
 *
 * "They come from the same function now" is exactly the kind of claim that stops being true
 * when somebody inlines a literal for a quick fix. So this reads the BUILT ARTIFACTS and
 * makes them agree with each other, which is a fact rather than an intention.
 */

const webApp = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function pagesIn(dir: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      found.push(...pagesIn(path));
    } else if (entry === "index.html") {
      found.push(path);
    }
  }
  return found;
}

const pages = pagesIn(PRODUCTION_DIR);
const headers = readFileSync(join(PRODUCTION_DIR, "_headers"), "utf8");

/** Every absolute http(s) origin `connect-src` names. */
const connectOrigins = [
  ...new Set(
    (/connect-src ([^;]*)/.exec(headers)?.[1] ?? "")
      .split(/\s+/)
      .filter((source) => /^https?:\/\//.test(source))
      .map((source) => new URL(source).origin),
  ),
];

function attr(html: string, selector: RegExp): string | null {
  return selector.exec(html)?.[1] ?? null;
}

describe("the origin this build is for", () => {
  it("is named by connect-src, once", () => {
    // ONE origin, not "at least one". Two would mean the policy permits a host nobody
    // decided on, and the count is knowable, so it is asserted rather than merely non-zero.
    expect(connectOrigins, `connect-src in _headers: ${connectOrigins.join(" ")}`).toHaveLength(1);
  });

  it("is stamped into every page, and there are pages to stamp", () => {
    // The denominator. A glob that matched nothing would satisfy every `for` below.
    expect(pages.length, "no built pages found, so this test compares nothing").toBeGreaterThan(5);
    for (const page of pages) {
      const html = readFileSync(page, "utf8");
      const stamp = attr(html, /<meta name="burrow-built-for" content="([^"]*)"/);
      expect(stamp, `${page.slice(PRODUCTION_DIR.length)} has no burrow-built-for stamp`).toBe(
        connectOrigins[0],
      );
    }
  });

  it("is the origin every canonical link points at — not localhost", () => {
    for (const page of pages) {
      const html = readFileSync(page, "utf8");
      const canonical = attr(html, /<link rel="canonical" href="([^"]*)"/);
      expect(
        canonical,
        `${page.slice(PRODUCTION_DIR.length)} has no canonical link`,
      ).not.toBeNull();
      expect(
        new URL(canonical as string).origin,
        `${page.slice(PRODUCTION_DIR.length)} is canonical at the wrong origin`,
      ).toBe(connectOrigins[0]);
    }
  });

  it("is the origin in each page's own CSP, not only in _headers", () => {
    // ADR 0014 §5 duplicates the policy into a `<meta>` because a static host may ignore
    // `_headers`. A build where the two disagreed would behave differently on two hosts for
    // reasons nobody could see from either one.
    for (const page of pages) {
      const html = readFileSync(page, "utf8");
      const csp = attr(html, /<meta http-equiv="Content-Security-Policy" content="([^"]*)"/);
      expect(csp, `${page.slice(PRODUCTION_DIR.length)} carries no CSP`).not.toBeNull();
      const origins = new Set(
        (/connect-src ([^;]*)/.exec(csp as string)?.[1] ?? "")
          .split(/\s+/)
          .filter((source) => /^https?:\/\//.test(source))
          .map((source) => new URL(source).origin),
      );
      expect([...origins]).toEqual(connectOrigins);
    }
  });

  it("is the origin the worker bundle fetches from", () => {
    // The fourth consumer, and the one with no HTML to read: the bundle carries absolute URLs
    // because a `blob:` worker's `self.location` is opaque.
    const engines = join(PRODUCTION_DIR, "engines");
    const bundle = readdirSync(engines).find((name) => /^burrow-worker\..*\.js$/.test(name));
    expect(bundle, "no worker bundle in the build").toBeDefined();
    const source = readFileSync(join(engines, bundle as string), "utf8");
    const manifest = /const BURROW_ENGINES = (\{[\s\S]*?\n\});/.exec(source)?.[1];
    expect(manifest, "the generated engine manifest is not in the bundle").toBeDefined();
    const parsed = JSON.parse(manifest as string) as Record<string, unknown>;

    expect(parsed.probeOrigin).toBe(connectOrigins[0]);
    const urls = Object.values(parsed)
      .filter(
        (value): value is { url: string } =>
          typeof value === "object" && value !== null && "url" in value,
      )
      .map((value) => value.url);
    expect(urls.length, "the manifest names no engine URLs").toBeGreaterThan(2);
    for (const url of urls) {
      expect(new URL(url).origin, `${url} is not on the build's origin`).toBe(connectOrigins[0]);
    }
  });

  it("is the origin the sitemap lists, for every page and no other host", () => {
    const sitemap = readFileSync(join(PRODUCTION_DIR, "sitemap.xml"), "utf8");
    const locs = [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map((m) => m[1]);

    for (const loc of locs) {
      expect(new URL(loc).origin, `${loc} is not on this build's origin`).toBe(connectOrigins[0]);
    }

    // THE SET, IN BOTH DIRECTIONS. The first version of this compared `locs.length` to
    // `pages.length` and then checked each `<loc>` was a page that exists -- which is
    // sitemap ⊆ build plus a count, and security review measured what that misses: replace
    // `/credits/` with a second copy of `/` and you have the right number of entries, every
    // one on the right origin, with a page silently absent. A count is satisfiable by
    // duplicates, so a count is not the defence against "4 of 15" that the comment claimed.
    //
    // Comparing sorted sets is both directions at once, and it names the route rather than
    // reporting that two numbers differ.
    const built = pages
      .map((path) => path.slice(PRODUCTION_DIR.length).replace(/index\.html$/, ""))
      .sort();
    const listed = locs.map((loc) => new URL(loc).pathname).sort();
    expect(listed, "the sitemap and the built pages are not the same set").toEqual(built);
  });

  it("is the origin robots.txt points a crawler at", () => {
    const robots = readFileSync(join(PRODUCTION_DIR, "robots.txt"), "utf8");
    const line = /^Sitemap:\s*(\S+)$/m.exec(robots);
    expect(line, "robots.txt names no sitemap, so nothing leads a crawler to one").not.toBeNull();
    expect(new URL(line?.[1] ?? "http://invalid.example").origin).toBe(connectOrigins[0]);
    // NOT `Disallow: /`. The whole site is static public pages; a build that quietly stopped
    // itself being indexed would look exactly like a build that was never crawled.
    expect(robots).not.toMatch(/^Disallow:\s*\/\s*$/m);
  });

  it("comes from ONE reader, so the ways it could disagree are not spelled out twice", () => {
    // A source-level assertion, and the only one here. The tests above would still pass if
    // somebody inlined the same literal in both places today — and that is exactly the state
    // this change removed, which held for months because it was correct on the day it was
    // written. What must stay true is that there is one reader.
    const config = readFileSync(join(webApp, "astro.config.mjs"), "utf8");
    const stage = readFileSync(
      resolve(webApp, "..", "..", "tools", "stage-web-engines.mjs"),
      "utf8",
    );
    for (const [name, source] of [
      ["astro.config.mjs", config],
      ["stage-web-engines.mjs", stage],
    ] as const) {
      expect(source, `${name} no longer reads the shared resolver`).toContain("resolveBuildOrigin");
      expect(
        source.includes("process.env.BURROW_SITE"),
        `${name} reads BURROW_SITE directly again; it should ask build-origin.mjs`,
      ).toBe(false);
    }
  });
});
