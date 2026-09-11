// The fail-closed guard, driven from the branch that must never happen.
//
// The worker checks `self.location.protocol === "blob:"` and refuses every file operation
// otherwise. That check exists because of a measured finding: a dedicated worker created
// from a same-origin *script URL* does not inherit the creating document's CSP, so it runs
// with no policy at all — and every other test in this suite would still pass, because they
// all exercise the blob path.
//
// So this test does the wrong thing on purpose: it constructs the worker from its plain URL
// and asserts it refuses to work. Without it, the guard is a line of code nothing executes,
// and a future refactor that reverted to `new Worker(url)` would restore the hole silently.

import { expect, test, type Page } from "@playwright/test";

interface ManifestEntry {
  url: string;
  integrity: string;
}

async function workerUrl(page: Page): Promise<string> {
  await page.goto("/harness");
  const raw = await page.getAttribute("#engines", "data-engines");
  if (!raw) {
    throw new Error("the harness page carries no engine manifest");
  }
  const manifest = JSON.parse(raw) as Record<string, ManifestEntry>;
  return manifest.worker.url;
}

test("a worker created from a plain URL refuses to touch a file", async ({ page }) => {
  const url = await workerUrl(page);

  const reply = await page.evaluate(async (workerScript) => {
    // `worker-src 'self' blob:` permits this, deliberately — the guard is in the worker, not
    // in the policy. A policy that forbade it would be a different (and weaker) design: it
    // would stop this page, and say nothing about a worker started from anywhere else.
    const worker = new Worker(workerScript);
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        worker.terminate();
        resolve({ timedOut: true });
      }, 15_000);
      worker.onmessage = (event) => {
        clearTimeout(timer);
        worker.terminate();
        resolve(event.data);
      };
      worker.postMessage({
        id: 1,
        op: "page_count",
        bytes: new Uint8Array([0x25, 0x50, 0x44, 0x46]).buffer,
        password: null,
        limits: {
          maxInputBytes: 1024,
          maxMemoryBytes: 1024 * 1024,
          maxDurationMs: 1000,
          maxPages: 10,
          maxPixels: 1000,
        },
      });
    });
  }, url);

  expect(reply, "the worker should have answered rather than hanging").not.toHaveProperty(
    "timedOut",
  );
  const typed = reply as { ok: boolean; kind: string; fatal: boolean; message: string };
  expect(typed.ok).toBe(false);
  expect(typed.kind).toBe("Internal");
  // Fatal, so a page that ignored the guard would at least discard the instance.
  expect(typed.fatal).toBe(true);
  // The message names the actual problem. This one is not input-derived — it is a fixed
  // constant about how the worker was constructed — so it is safe to be specific.
  expect(typed.message).toContain("blob:");
});

test("the same worker, constructed from a Blob, does the work", async ({ page }) => {
  // The control. Without it, a worker that refused everything unconditionally — or one that
  // failed to load at all — would pass the test above and look like a working guard.
  const url = await workerUrl(page);

  const ready = await page.evaluate(async (workerScript) => {
    const response = await fetch(workerScript);
    const source = await response.text();
    const blobUrl = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
    const worker = new Worker(blobUrl);
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        worker.terminate();
        resolve(false);
      }, 60_000);
      worker.onmessage = (event) => {
        clearTimeout(timer);
        worker.terminate();
        resolve(event.data.ready === true);
      };
      worker.postMessage({ id: 1, type: "init" });
    });
  }, url);

  expect(ready, "a blob: worker must initialise").toBe(true);
});
