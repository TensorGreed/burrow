---
name: m2-r8-r9-specs
description: R8/R9 named checks (5c3c174, #137) — measured bypasses of the recorder/predicates (foreign id, non-binary content, deferred exits, navigator.locks, nested worker via location.href) and the ADR claim they refute
metadata:
  type: project
---

Reviewed 2026-09-27 at 5c3c174 (branch test/137-r8-r9-specs). The shipped worker is clean; the checks are weaker than the ADR note says.

- **Structural fact that keeps this non-exploitable today:** `wasm_bindgen.redact` is one sync Rust call that verifies internally, so JS never holds unverified output. Every plant (theirs and mine) leaks the *input*, as a proxy. The checks matter when #206 or a streaming API gives JS pre-verification bytes.
- **R8 bypasses measured in all 3 browsers:** bytes under any id that is neither null nor the request's (0, id+1000, `undefined`) pass every rule; content as number array or string passes (countBytes counts only binary types); MessagePort transfer passes; ImageData passes in Firefox only (counted in Chromium/WebKit); a second copy of the document posted 200 ms after the reply is absent from the log when the spec reads it.
- **R9 bypasses measured:** `setTimeout(exit, 0)` before the call fires after the reply (await between call and post runs microtasks only); `navigator.locks.request(name-with-bytes)` is unstubbed and the page reads it via `locks.query()`; `new Worker(self.location.href)` builds a nested worker with no createObjectURL call, whose stub reports go to the parent and are dropped, and wrote input to IndexedDB the page read back. That refutes ADR 0006's "the only way to get a blob: URL is createObjectURL".
- The two-documents reply IS plantable via `mutate` (protocol text is in the bundle), contrary to the spec comment.
- No R9 case runs a refusal; r9Violations cannot express a clean refusal (returns "no verified reply").
- Production reach: harness-driver lives only in dist/host, removed by astro:build:done; production-build.test.ts asserts. Sound.
- Method: symlinked pkg*/vendor/node_modules into a worktree, ports 4471/4472, one zz-bypass.spec.ts reusing the shipped predicates. Baseline 27/27, bypass 34/36 (ImageData failed in 2).

Related: [[m2-redaction-binding-137]], [[m2-web-redaction-impl]].
