#!/usr/bin/env node
// Stage the wasm engine artifacts into the web app, and generate the CSP that permits
// exactly those files and nothing else.
//
// WHY THIS IS GENERATED RATHER THAN WRITTEN BY HAND
//
// ADR 0006 requirement 3 asks for two things that, taken literally, cancel out: supply
// `Module.wasmBinary` *and* ship `connect-src 'none'`. Supplying the bytes ourselves means
// fetching them, and fetch is exactly what `connect-src` governs -- from the worker too,
// since a dedicated worker inherits the owner document's policy. There is no directive
// under which a .wasm file can be fetched while connect-src is 'none'.
//
// So the policy is narrowed instead of loosened: `default-src 'none'`, every directive
// explicit, no cross-origin source anywhere, and `connect-src` naming the **exact
// content-hashed engine URLs**. A CSP path without a trailing slash matches exactly, so
// those two URLs are the only things any code on the page may fetch. The engines are loaded
// with `fetch(url, { integrity })`, so the bytes are pinned as well as the origin.
//
// Two consequences worth stating plainly:
//
//   1. CSP ignores query strings. `/engines/pdfium.<hash>.wasm?leak=<bytes>` is permitted by
//      the policy. The browser therefore enforces "no cross-origin requests"; it does NOT
//      enforce "no exfiltration". What closes that is the zero-requests-after-init test in
//      PR 4a-ii. Neither mechanism is sufficient alone and the ADR says so.
//   2. A CSP source expression needs a host -- `'self'` takes no path -- so the policy must
//      name an absolute origin, which means it is generated per build. That makes a build
//      origin-bound. It is the right trade here: the policy cannot be loosened by
//      redeploying somewhere else.
//
// Inlining the engines as base64 was considered and rejected: it closes only the fetch
// channel, leaves `script-src 'self'` open regardless, and gives up both integrity checking
// and streaming compile. See docs/adr/0014-web-engine-loading-and-csp.md.
//
// THE WORKER IS BUNDLED INTO ONE FILE, AND THAT IS A SECURITY PROPERTY
//
// Measured: a dedicated worker created from a same-origin script URL does NOT inherit the
// creating document's CSP -- it takes its policy from that script's HTTP response headers,
// and a static host sends none. The worker ran with no policy at all, and a cross-origin
// fetch from inside it reached the network, while every page-level CSP test passed.
//
// Only blob:/data:/about: workers inherit. So the page fetches the worker source with
// `integrity` and constructs the worker from a Blob. Two things follow:
//
//   * All worker code must be in ONE file, because the page fetches one source text. That
//     is a gain, not a cost: `importScripts` has no integrity mechanism, so the three glue
//     files -- including 160 KB of third-party PDFium glue that owns the heap the bridge
//     writes to -- were previously loaded unverified. Now one digest covers everything.
//   * The engine manifest is generated INTO the bundle rather than sent by postMessage:
//     `pdfium.js` begins instantiating as it is parsed, so there is no moment after load
//     and before instantiation at which a message could arrive.

import { createHash } from "node:crypto";
import { mkdir, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..");
const webApp = join(repo, "apps", "web");
const outDir = join(webApp, "public", "engines");
const generatedDir = join(webApp, "src", "generated");

// The origin the policy is written against. A build for a different origin is a different
// build; see the header.
const ORIGIN = (() => {
  const raw = process.env.BURROW_SITE ?? "http://localhost:4321";
  // Round-tripped through `URL` rather than interpolated raw. This value reaches a CSP
  // directive and a `_headers` file; a newline in it would inject arbitrary response
  // headers on a host that reads one, and a trailing slash would silently produce a policy
  // that blocks every engine. `.origin` strips path, query, fragment and any trailing
  // slash, and the constructor rejects anything that is not a URL at all.
  let parsed;
  try {
    parsed = new URL(raw);
  } catch {
    console.error(`stage-web-engines: BURROW_SITE is not a valid URL: ${JSON.stringify(raw)}`);
    process.exit(1);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    console.error(`stage-web-engines: BURROW_SITE must be http(s), got ${parsed.protocol}`);
    process.exit(1);
  }
  return parsed.origin;
})();

// The four files the worker loads. `.js` is Emscripten glue, `.wasm` is the module.
const ENGINE_FILES = [
  { id: "pdfiumWasm", from: "pdfium.wasm", kind: "wasm", source: "engines" },
  { id: "qpdfWasm", from: "qpdf.wasm", kind: "wasm", source: "engines" },
  // burrow's own module is staged and fetched the same way. It is not an "engine", but it
  // is a .wasm the worker fetches, and connect-src governs that fetch identically -- so
  // leaving it out would mean either a looser policy or a module that cannot load.
  { id: "burrowWasm", from: "burrow_wasm_bg.wasm", kind: "wasm", source: "pkg" },
];

/** The same manifest with page-relative URLs rewritten against the build's origin. */
function absolute(manifest) {
  return Object.fromEntries(
    Object.entries(manifest).map(([id, entry]) => [id, { ...entry, url: `${ORIGIN}${entry.url}` }]),
  );
}

function sriFor(bytes) {
  return `sha384-${createHash("sha384").update(bytes).digest("base64")}`;
}

function contentHash(bytes) {
  // 16 hex characters: enough that a collision is not a practical concern for cache
  // busting, short enough to keep the CSP header readable.
  return createHash("sha256").update(bytes).digest("hex").slice(0, 16);
}

/**
 * The Content-Security-Policy, built around the two engine URLs.
 *
 * Every directive is listed even where `default-src 'none'` would already cover it. That is
 * deliberate: a reader should be able to see what is permitted without knowing which
 * directives fall back to default-src, and a future directive that does not fall back
 * cannot quietly open a hole.
 */
function policyFor(wasmUrls, { forMeta }) {
  const directives = [
    "default-src 'none'",
    // 'wasm-unsafe-eval' is what permits WebAssembly compilation at all. It does not permit
    // eval() of JavaScript -- that would be 'unsafe-eval', which is absent.
    "script-src 'self' 'wasm-unsafe-eval'",
    // `blob:` is required, and is the narrowest form of the thing it permits: the blob's
    // content is this site's own worker bundle, fetched from a connect-src entry with its
    // integrity pinned. Without it the worker cannot inherit this policy at all, and runs
    // unpoliced -- which is the failure this whole arrangement exists to fix.
    //
    // `script-src` deliberately does NOT get `blob:`. The worker is constructed from a
    // Blob; no script is ever *loaded* from one.
    "worker-src 'self' blob:",
    // The whole point. Exactly the engine modules, by exact path, same origin.
    `connect-src ${wasmUrls.join(" ")}`,
    "style-src 'self'",
    "img-src 'self'",
    "font-src 'self'",
    "base-uri 'none'",
    "form-action 'none'",
    "object-src 'none'",
  ];

  // `frame-ancestors` is IGNORED when delivered in a <meta> element -- the browser says so
  // out loud, in the console, on every page load. Including it there would be noise that
  // teaches a reader to ignore CSP console errors, which is worse than not having it. It
  // goes in the header, where it works.
  if (!forMeta) {
    directives.push("frame-ancestors 'none'");
  }

  return directives.join("; ");
}

async function main() {
  const arch = process.env.BURROW_ENGINE_ARCH ?? "wasm";
  const sources = {
    engines: join(repo, "engines", "vendor", arch, "lib"),
    pkg: join(repo, "bindings", "burrow-wasm", "pkg"),
  };

  if (!existsSync(sources.engines)) {
    console.error(`stage-web-engines: ${sources.engines} does not exist.`);
    console.error("  Run engines/fetch.sh && engines/build-wasm.sh first.");
    process.exit(1);
  }
  if (!existsSync(sources.pkg)) {
    console.error(`stage-web-engines: ${sources.pkg} does not exist.`);
    console.error("  Run: wasm-pack build bindings/burrow-wasm --target no-modules --out-dir pkg --release");
    console.error("  (no-modules, not web: the classic worker pdfium.js requires cannot import an ES module.)");
    process.exit(1);
  }

  // Start clean, so a renamed artifact from a previous build cannot linger in `dist/` and
  // be served alongside the current one.
  await rm(outDir, { recursive: true, force: true });
  await mkdir(outDir, { recursive: true });
  await mkdir(generatedDir, { recursive: true });

  const staged = {};
  const wasmUrls = [];

  for (const file of ENGINE_FILES) {
    const source = join(sources[file.source], file.from);
    if (!existsSync(source)) {
      console.error(`stage-web-engines: ${source} is missing.`);
      console.error(
        file.source === "pkg"
          ? "  Run: wasm-pack build bindings/burrow-wasm --target no-modules --out-dir pkg --release"
          : "  Run engines/build-wasm.sh -- it builds qpdf.js/qpdf.wasm and unpacks pdfium.",
      );
      process.exit(1);
    }
    const bytes = await readFile(source);
    const hash = contentHash(bytes);
    const name = file.from.replace(/\.(js|wasm)$/, `.${hash}.$1`);
    await writeFile(join(outDir, name), bytes);

    const path = `/engines/${name}`;
    staged[file.id] = {
      url: path,
      integrity: sriFor(bytes),
      bytes: bytes.length,
    };
    if (file.kind === "wasm") {
      wasmUrls.push(`${ORIGIN}${path}`);
    }
    console.log(`  ${file.from} -> ${name}  (${bytes.length} bytes)`);
  }

  // ---- the worker bundle ----------------------------------------------------------
  //
  // Order is load-bearing. `prelude.js` assigns `self.Module`, which `pdfium.js` reads at
  // load time; `main.js` runs last because everything it uses must already exist.
  const workerSource = [
    "// GENERATED by tools/stage-web-engines.mjs. Do not edit.\n",
    // ABSOLUTE urls, not the page-relative ones. A blob: worker's `self.location` is an
    // opaque blob: URL with no useful base, so `fetch("/engines/...")` inside it fails with
    // "Failed to parse URL" -- it cannot resolve a relative reference at all. This is the
    // same reason `locateFile` is never allowed to run and the compiled modules are handed
    // in directly.
    //
    // It also means the worker bundle is origin-bound in the same way the CSP is, which is
    // consistent: both are generated against BURROW_SITE and neither can be relocated
    // without a rebuild.
    `const BURROW_ENGINES = ${JSON.stringify(absolute(staged), null, 2)};\n`,
    await readFile(join(webApp, "src/worker/prelude.js"), "utf8"),
    await readFile(join(webApp, "src/worker/bridge.js"), "utf8"),
    // qpdf is MODULARIZE'd: it defines one function and touches nothing else.
    await readFile(join(sources.engines, "qpdf.js"), "utf8"),
    // PDFium is NOT modularised -- its state lives in worker globals, which is exactly why
    // it must appear once and why `self.Module` has to be set before this point.
    await readFile(join(sources.engines, "pdfium.js"), "utf8"),
    // wasm-bindgen `--target no-modules`: defines the `wasm_bindgen` global.
    await readFile(join(sources.pkg, "burrow_wasm.js"), "utf8"),
    await readFile(join(webApp, "src/worker/main.js"), "utf8"),
  ].join("\n");

  const workerBytes = Buffer.from(workerSource, "utf8");
  const workerName = `burrow-worker.${contentHash(workerBytes)}.js`;
  await writeFile(join(outDir, workerName), workerBytes);
  const workerUrl = `/engines/${workerName}`;
  staged.worker = {
    url: workerUrl,
    integrity: sriFor(workerBytes),
    bytes: workerBytes.length,
  };
  console.log(`  worker bundle -> ${workerName}  (${workerBytes.length} bytes)`);

  // The page fetches the worker's SOURCE TEXT, so its URL is a connect-src entry like any
  // engine. It is not a `script-src` entry: it is never loaded as a script from that URL.
  const fetchable = [...wasmUrls, `${ORIGIN}${workerUrl}`];

  const metaPolicy = policyFor(fetchable, { forMeta: true });
  const headerPolicy = policyFor(fetchable, { forMeta: false });

  const generated = `// GENERATED by tools/stage-web-engines.mjs. Do not edit.
//
// Regenerate with: pnpm prebuild  (or node ../../tools/stage-web-engines.mjs)
//
// The URLs are content-hashed, so a changed engine is a changed URL and a changed CSP. The
// integrity strings are what the worker passes to fetch(), so a byte-level change to an
// engine fails the load rather than silently running different code.

export const ENGINE_ORIGIN = ${JSON.stringify(ORIGIN)};

export const ENGINES = ${JSON.stringify(staged, null, 2)};

/** The Content-Security-Policy this build is served under. */
export const CSP = ${JSON.stringify(metaPolicy)};

/**
 * The policy a host should send as a header.
 *
 * Identical to \`CSP\` plus \`frame-ancestors\`, which a <meta> element cannot carry --
 * browsers ignore it there and log an error for it.
 */
export const CSP_HEADER = ${JSON.stringify(headerPolicy)};
`;

  await writeFile(join(generatedDir, "engines.js"), generated);

  // A _headers file for hosts that read one (Netlify, Cloudflare Pages). The meta tag in
  // BaseLayout is what actually enforces the policy in every environment including local
  // preview and Playwright; this is belt and braces for a real deployment, where a header
  // also covers responses that are not HTML.
  await writeFile(
    join(webApp, "public", "_headers"),
    `# GENERATED by tools/stage-web-engines.mjs. Do not edit.\n/*\n  Content-Security-Policy: ${headerPolicy}\n  X-Content-Type-Options: nosniff\n  Referrer-Policy: no-referrer\n`,
  );

  console.log(`\n  origin: ${ORIGIN}`);
  console.log(`  CSP:    ${metaPolicy}`);
  console.log(`  header: + frame-ancestors 'none'`);

  const listing = await readdir(outDir);
  console.log(`\n  staged ${listing.length} files into apps/web/public/engines/`);
}

await main();
