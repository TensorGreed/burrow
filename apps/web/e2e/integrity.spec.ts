// The engine bytes are integrity-pinned, and the pin is load-bearing.
//
// `connect-src` pins *where* the engines come from. Subresource integrity pins *what* they
// are. Without the second, a host that served a modified `pdfium.wasm` — a compromised CDN,
// a tampered deploy, a cache-poisoning proxy — would be serving code that runs over the
// user's files, from an origin the policy trusts.
//
// FOUR ARTIFACTS, AND ALL THE WORKER'S CODE IS IN ONE OF THEM.
//
// An earlier shape staged the three Emscripten `.js` glue files separately and loaded them
// with `importScripts`, which has no integrity mechanism — so 160 KB of third-party PDFium
// glue, which owns the heap the bridge writes to, ran unverified while the manifest carried
// a digest for it that nothing checked. Bundling all worker code into one file (done for the
// blob: worker, see ADR 0014) makes that one digest cover every line of it.
//
// This is also the half that inlining the engines as base64 would have thrown away, which
// is recorded in ADR 0014 as a reason that option was rejected.

import { expect, test, type Page } from "@playwright/test";

interface Entry {
  url: string;
  integrity: string;
  bytes: number;
}

/** The engine manifest the page was built with. */
async function readManifest(page: Page): Promise<Record<string, Entry>> {
  const raw = await page.getAttribute("#engines", "data-engines");
  if (!raw) {
    throw new Error("the harness page carries no engine manifest");
  }
  return JSON.parse(raw) as Record<string, Entry>;
}

test("the manifest pins every engine artifact by digest", async ({ page }) => {
  await page.goto("/harness");

  // Read the `data-engines` attribute, not the rendered `#result`: the latter is only
  // filled in once the engines have initialised, and this test is about the manifest
  // itself, not about the load succeeding.
  const engines = await readManifest(page);

  const ids = Object.keys(engines).sort();
  expect(ids).toEqual(["burrowWasm", "control", "pdfiumWasm", "qpdfWasm", "worker"]);

  // The guard's control resource. Small on purpose: it is fetched `cache: "no-store"` at
  // every worker start, and it exists only to prove an allowlisted request succeeds.
  expect(engines.control.bytes, "the control should be a few bytes, not a payload").toBeLessThan(
    1024,
  );
  expect(engines.control.url).toMatch(/^\/engines\/control\.[0-9a-f]{16}\.txt$/);

  for (const [id, entry] of Object.entries(engines)) {
    // sha384 rather than sha256: it is the SRI default for a reason, and there is no cost.
    expect(entry.integrity, `${id} integrity`).toMatch(/^sha384-[A-Za-z0-9+/]+=*$/);
    // Content-hashed, so a changed engine is a changed URL — which is also what makes the
    // generated CSP change with it.
    expect(entry.url, `${id} url`).toMatch(/^\/engines\/.+\.[0-9a-f]{16}\.(js|wasm|txt)$/);
    expect(entry.bytes, `${id} size`).toBeGreaterThan(0);
  }
});

test("the worker bundle contains all worker code, so one digest covers it", async ({ page }) => {
  await page.goto("/harness");
  const engines = await readManifest(page);

  // The bundle carries the prelude, the bridge, both Emscripten glues, the wasm-bindgen glue
  // and the protocol. If a future change split any of that back out into its own
  // `importScripts`, this size floor would fail — and so would the guarantee, silently,
  // because `importScripts` cannot be integrity-checked.
  expect(
    engines.worker.bytes,
    "the worker bundle looks too small to contain the glue",
  ).toBeGreaterThan(150_000);

  const source = await page.evaluate(async (url) => {
    const response = await fetch(url);
    return response.text();
  }, engines.worker.url);

  for (const marker of [
    "BURROW_ENGINES", // the generated manifest
    "__burrow_pdfium_load", // the bridge
    "createQpdfModule", // the qpdf glue
    "FPDF_LoadMemDocument64", // the pdfium glue
    "wasm_bindgen", // the Rust glue
    "INHERITS_PAGE_CSP", // the fail-closed guard
  ]) {
    expect(source, `the bundle is missing ${marker}`).toContain(marker);
  }
});

test("a wrong digest fails the load rather than running the bytes anyway", async ({ page }) => {
  await page.goto("/harness");

  const engines = await readManifest(page);
  const target = engines.qpdfWasm;

  // The real fetch the worker performs, with one character of the digest changed. If SRI
  // were not enforced — a wrong `integrity` value, or a `crossorigin` mistake that made the
  // browser skip the check — this would resolve and the test would fail, which is the whole
  // point of doing it against the *actual* URL rather than a synthetic one.
  const tampered = target.integrity.replace(/.$/, (last) => (last === "A" ? "B" : "A"));
  expect(tampered).not.toBe(target.integrity);

  const rejected = await page.evaluate(
    async ([url, integrity]) => {
      try {
        const response = await fetch(url, { integrity });
        // The check can also surface as a failure while the body is read.
        await response.arrayBuffer();
        return false;
      } catch {
        return true;
      }
    },
    [target.url, tampered] as const,
  );

  expect(rejected, "a corrupted engine must not load").toBe(true);

  // And the control: the correct digest still works, so the test above is measuring the
  // digest rather than something incidental about the request.
  const accepted = await page.evaluate(
    async ([url, integrity]) => {
      try {
        const response = await fetch(url, { integrity });
        const bytes = await response.arrayBuffer();
        return bytes.byteLength > 0;
      } catch {
        return false;
      }
    },
    [target.url, target.integrity] as const,
  );

  expect(accepted, "the genuine engine must still load").toBe(true);
});
