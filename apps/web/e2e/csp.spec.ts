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

import { openHarness, type ProbeResult } from "./harness";

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

/**
 * The worker is where the file bytes are, and it was the half that was unprotected.
 *
 * A dedicated worker created from a same-origin *script URL* does not inherit the creating
 * document's CSP — it takes its policy from that script's HTTP response headers, and a static
 * host sends none. Measured during 4a-i: the worker ran with no policy at all and a
 * cross-origin fetch from inside it reached the network, while every page-level test above
 * passed. So the worker is now constructed from a Blob, which does inherit.
 *
 * **Asserted differentially, not by the violation event.** An allowlisted request must
 * succeed and a non-allowlisted one must be refused; together those separate "the policy
 * refused it" from "the network is broken" using only whether requests succeed. The earlier
 * version required a `securitypolicyviolation` event — and **WebKit does not dispatch one in
 * a worker**, so it reported no violation for requests it had correctly refused. The guard in
 * the worker made the same mistake and refused every operation in WebKit; this is the test
 * shaped like the fix.
 */
test("the policy applies inside the worker, and is what blocks the request", async ({ page }) => {
  await openHarness(page);

  // ABSOLUTE same-origin URLs. A relative one cannot be used here at all: a blob: worker's
  // `self.location` is an opaque blob: URL, so `fetch("/")` fails to parse before CSP is
  // consulted — which would look like a pass for entirely the wrong reason.
  const origin = new URL(page.url()).origin;

  // THE CONTROL. An allowlisted engine URL, fetched from inside the same worker. If this
  // were blocked too, every assertion below would be satisfied by a broken network rather
  // than by a policy.
  const allowlisted = await page.evaluate(async () => {
    const manifest = JSON.parse(
      document.getElementById("engines")?.getAttribute("data-engines") ?? "{}",
    ) as Record<string, { url: string }>;
    return window.burrowHarness.fetchFromWorker(
      new URL(manifest.qpdfWasm.url, location.origin).href,
    );
  });
  expect(allowlisted.failed, "the probe worker failed to start").toBeFalsy();
  expect(
    allowlisted.blocked,
    "an allowlisted engine URL must NOT be blocked — otherwise this proves nothing",
  ).toBe(false);

  for (const target of [
    "https://example.com/collect",
    "https://cdn.jsdelivr.net/npm/anything",
    `${origin}/`,
    `${origin}/engines/not-an-engine.wasm`,
  ]) {
    const result: ProbeResult = await page.evaluate(
      (url) => window.burrowHarness.fetchFromWorker(url),
      target,
    );

    expect(result.failed, `${target}: the probe worker failed to start`).toBeFalsy();
    expect(result.blocked, `${target} should be refused inside the worker`).toBe(true);
  }
});

/**
 * Which browsers report a policy violation inside a worker.
 *
 * Informational, and deliberately not part of the guarantee above — but asserted rather than
 * commented, so that "WebKit does not dispatch this" stays a measured fact. Chromium and
 * Firefox do; if WebKit gains it, this test says so rather than quietly passing.
 */
test("securitypolicyviolation reaches the worker in Chromium and Firefox", async ({
  page,
}, testInfo) => {
  await openHarness(page);
  const origin = new URL(page.url()).origin;

  const result: ProbeResult = await page.evaluate(
    (url) => window.burrowHarness.fetchFromWorker(url),
    `${origin}/engines/not-an-engine.wasm`,
  );
  expect(result.blocked, "the request must be refused regardless").toBe(true);

  const dispatched = result.violations.some((v) => v.includes("connect-src"));
  if (testInfo.project.name === "webkit") {
    // Not asserted false: this is a gap, not a requirement. Recorded so the differential
    // check above is understood to be load-bearing rather than belt-and-braces.
    testInfo.annotations.push({
      type: "browser-gap",
      description: `WebKit dispatched ${dispatched ? "a" : "no"} securitypolicyviolation in the worker`,
    });
    return;
  }
  expect(
    dispatched,
    `expected a connect-src violation, got ${JSON.stringify(result.violations)}`,
  ).toBe(true);
});

test("the worker refuses to touch a file unless it inherits the policy", async ({ page }) => {
  await openHarness(page);

  // The fail-closed guard, from the other side. The worker checks
  // `self.location.protocol === "blob:"` and refuses every operation otherwise, so a future
  // refactor that constructs it from a plain URL — losing the CSP entirely — breaks loudly
  // instead of silently running unpoliced with every test still green.
  //
  // Here the worker IS a blob, so it must accept work. `e2e/worker-guard.spec.ts` drives the
  // other branch.
  // No cast: `./harness` re-exports `src/host/harness-api.d.ts`, whose `Window` augmentation
  // is in scope for every callback in this file. The inline `as unknown as { ... }` that used
  // to be here is the one `e2e/harness.ts`'s header describes in the past tense.
  const ready = await page.evaluate(() => window.burrowHarness.workerInheritsCsp());
  expect(ready, "a blob: worker must accept work").toBe(true);
});
