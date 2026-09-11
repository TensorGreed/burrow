// What the Content-Security-Policy actually enforces, measured rather than asserted.
//
// The brief for this PR asks Playwright to prove the CSP blocks a deliberate `fetch` both
// to another origin and to a same-origin URL. It does the first. **It does not do the
// second, and cannot** — and finding that out is the reason ADR 0014 exists.
//
// `connect-src` names the exact content-hashed engine URLs. A CSP path matches exactly when
// it has no trailing slash, so `/engines/other.wasm` is blocked and `/anything` is blocked.
// But **CSP ignores query strings**: `/engines/qpdf.<hash>.wasm?leak=<bytes>` matches the
// permitted source and is allowed through. That is not a gap in this policy, it is how the
// spec defines matching, and no policy that permits the engines to load can close it.
//
// So the guarantee is split, and both halves are load-bearing:
//
//   * **No cross-origin request** — enforced by the browser. Tested here.
//   * **No request of any kind after engine init** — enforced by test. That is 4a-ii's
//     zero-requests test, which watches every request type while files are processed.
//
// Claiming the browser closes the same-origin channel would be the comfortable version and
// it would be false.

import { expect, test, type Page } from "@playwright/test";

async function openHarness(page: Page) {
  await page.goto("/harness");
  await expect(page.locator("#status")).toHaveText("engines ready", { timeout: 60_000 });
}

/** Try a fetch from the page and report whether the browser refused it. */
async function fetchIsBlocked(page: Page, url: string): Promise<boolean> {
  return page.evaluate(async (target) => {
    try {
      await fetch(target, { mode: "no-cors" });
      return false;
    } catch {
      // A CSP refusal rejects the promise. So does a network error, which is why the
      // cross-origin target below is a port nothing listens on *and* a different origin:
      // either way the request did not reach anyone, which is the property under test.
      return true;
    }
  }, url);
}

test("a cross-origin request is blocked by the browser", async ({ page }) => {
  await openHarness(page);

  for (const target of [
    "https://example.com/collect",
    "http://127.0.0.1:9/collect",
    "https://cdn.jsdelivr.net/npm/anything",
  ]) {
    expect(await fetchIsBlocked(page, target), `${target} should be refused`).toBe(true);
  }
});

test("a same-origin URL that is not an engine is blocked", async ({ page }) => {
  await openHarness(page);

  // `connect-src` lists exact paths, so everything else on the origin is refused — which
  // is considerably stronger than `connect-src 'self'` would have been.
  for (const target of ["/", "/harness", "/engines/", "/engines/not-an-engine.wasm"]) {
    expect(await fetchIsBlocked(page, target), `${target} should be refused`).toBe(true);
  }
});

test("the query-string hole is real, and is why the zero-requests test exists", async ({
  page,
}) => {
  await openHarness(page);

  // Read the permitted engine URL out of the policy the page is actually running under,
  // rather than recomputing it — if the generator and the layout ever disagree, this test
  // should follow the browser.
  const permitted = await page.evaluate(() => {
    const meta = document.querySelector('meta[http-equiv="Content-Security-Policy"]');
    const content = meta?.getAttribute("content") ?? "";
    const directive = content.split(";").find((d) => d.trim().startsWith("connect-src"));
    return directive?.trim().split(/\s+/)[1] ?? "";
  });
  expect(permitted).toMatch(/\/engines\/.*\.wasm$/);

  // THIS IS EXPECTED TO SUCCEED. It documents the limit of the mechanism, so that nobody
  // later reads the two tests above and concludes the browser has closed every channel.
  // If CSP semantics ever change so that this is refused, this test fails and the ADR and
  // the comments above should be revisited — a test that notices good news too.
  const allowed = !(await fetchIsBlocked(page, `${permitted}?exfiltrated=secret`));
  expect(
    allowed,
    "CSP ignores query strings, so this is permitted. The zero-requests-after-init test in 4a-ii is what closes it.",
  ).toBe(true);
});

test("the policy has no cross-origin source anywhere in it", async ({ page }) => {
  await page.goto("/harness");
  const policy = await page.evaluate(() => {
    const meta = document.querySelector('meta[http-equiv="Content-Security-Policy"]');
    return meta?.getAttribute("content") ?? "";
  });

  expect(policy).toContain("default-src 'none'");
  expect(policy).toContain("object-src 'none'");
  expect(policy).toContain("base-uri 'none'");
  expect(policy).toContain("form-action 'none'");
  // `frame-ancestors` is deliberately NOT here: a browser ignores it in a <meta> element
  // and logs an error for it on every page load. It goes in `public/_headers`, where it
  // works -- see `src/production-build.test.ts`, which asserts the header file carries it.
  expect(policy).not.toContain("frame-ancestors");

  // 'wasm-unsafe-eval' permits WebAssembly compilation. 'unsafe-eval' would permit eval()
  // of JavaScript and must never appear; nor must 'unsafe-inline'.
  expect(policy).toContain("'wasm-unsafe-eval'");
  expect(policy).not.toContain("'unsafe-eval'");
  expect(policy).not.toContain("'unsafe-inline'");

  // Every host in the policy must be this origin. A wildcard or a third party would defeat
  // the whole arrangement, and is the single easiest thing to add by accident.
  const origin = new URL(page.url()).origin;
  const hosts = policy.match(/https?:\/\/[^\s;]+/g) ?? [];
  expect(hosts.length, "the engine URLs should be the only absolute sources").toBeGreaterThan(0);
  for (const host of hosts) {
    expect(host.startsWith(origin), `${host} is not same-origin`).toBe(true);
  }
  expect(policy).not.toMatch(/[\s;]\*[\s;]|\*\./);
});
