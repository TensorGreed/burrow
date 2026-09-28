---
name: m2-r8-r9-specs
description: R8/R9 named checks (#137) — round 1 (5c3c174) and round 2 (9d9ec66) measured bypasses; the checks describe message SHAPE, so any value type the walkers skip carries content
metadata:
  type: project
---

**Round 1, 5c3c174 (2026-09-27).** Shipped worker clean; checks weaker than ADR note.
- Non-exploitable today: `wasm_bindgen.redact` is one sync call that verifies internally, so JS never holds unverified output. Plants leak the *input* as a proxy. The checks matter when #206 or a streaming API gives JS pre-verification bytes.
- R8 bypasses: foreign/undefined id, number array/string, MessagePort, ImageData (Firefox), a copy 200 ms after the reply. R9: setTimeout-deferred exit, navigator.locks, `new Worker(self.location.href)`.

**Round 2, 9d9ec66 (same day).** Every round-1 plant is now caught in all 3 browsers (shipped suite 75/75 plus 3 SharedWorker skips; my own re-plants of id 0, string id and a 200 ms copy are caught too). New bypasses, each measured passing BOTH real-worker assertions (bytes-count==1, r8Violations==[]) in chromium/firefox/webkit:
- `harnessOwn` skips ANY single-key `__burrow*` message: `{__burrowLeak: Array.from(bytes)}`, `{__burrowSettled: [...]}`. A binary value is caught only by the success test's count, not by r8Violations.
- Values the walkers don't see: object KEYS, BigInt, Error message, boxed String (longest=1), a transferred ReadableStream (0 bytes, 0 ports; transfers in all 3). 64-char chunks: 321 `{starting}` messages pass, because shape counts are not checked.
- The terminal reply's non-string fields are unchecked: the input as `rotations` passes (done with a Proxy on the reply at the call line).
- Settle can be forged (`{__burrowSettled:true}` at 50 ms after the reply) or sped up (the prologue reads global `setTimeout` at event time, so a hijack makes it 0). Either way a copy posted at 300–400 ms is missed.
- R9: the stubs' `tell` reads `self.postMessage` at call time, so overriding it silences every report. `Worker.prototype.constructor` is the un-proxied original (nested worker ran, trap count 0). `fetch(<connect-src wasm>?leak=hex)` reached the server log in all 3; zero-requests.spec has no redaction case. `webkitURL` is absent in all worker scopes, so it is not a bypass.
- Method: worktree + symlinks, ports 4481/4482, zz-bypass2.spec.ts reusing the shipped predicates; a separate throwaway-worker probe for the Proxy, webkitURL and stream facts.

Related: [[m2-redaction-binding-137]], [[m2-web-redaction-impl]].
