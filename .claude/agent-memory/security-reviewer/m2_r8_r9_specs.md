---
name: m2-r8-r9-specs
description: R8/R9 named checks (#137) — rounds 1-5 (5c3c174..d00aa49); r5: all r4 plants caught 3/3; a bundle-posted armed list at index 0 (no-stub runs) passes the whole doc through R8; 564 B numeric measured
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

**Round 3, cf4bf34 (2026-09-28).** Deny-by-default `shapeOf`. Every r2 bypass re-planted is caught 3/3 (shipped suite 150 pass + 3 skips). New, measured in all 3 browsers unless noted:
- `shapeOf` encodes an array by ELEMENT types only; structured clone carries an array's named props (probe: 3/3). Whole 20 kB input as 48-char props of the reply's `rotations` passes (success AND refusal); a 65-char prop is flagged, so they do arrive.
- `harnessOwn` accepts `{__burrowSideChannelArmed: []}` any number of times, at any time, even with no stubs armed: the whole doc in 5 such messages passes.
- The unused `fatal` EARLY slot can be posted after the reply (64 UTF-16 chars + a double). EARLY has no position rule.
- In-bounds budget per op, passing: 9 strings × 64 UTF-16 units (lone surrogates survive) + 4 doubles (NaN payloads survive) + 64-double rotations ≈ 1.7 kB. Secrets redaction removes are < 64 chars.
- Firefox + WebKit fire a later-registered CAPTURE listener before SETTLE's bubble one at the worker global; Chromium does not. So there a bundle reads the nonce, stops SETTLE and echoes at once; a copy 700 ms later passes (control without the forge is caught).
- Network half is tight in practice (all 4 redact URLs are fetched in-window, so any 5th is "twice"; CSP pins exact URLs). But `networkViolations` ignores origin and method and admits any 16-hex hash: a hand log of foreign-origin POSTs to `qpdf.0000…` passes. Matters only if CSP regresses.
- Method note: Playwright's page.evaluate return drops array named props — witness them through the recorder, not the returned reply.

**Round 4, 2930dfb (2026-09-28).** Field rules + capture-first settle. All 14 r3 re-plants caught 3/3 (named props success+refusal, forged armed lists, fatal init after reply, 64-char enums, NaN/fraction, 64 doubles, lone surrogate, capture forge + 900 ms copy, 321 starting). A later capture listener and an onmessage wrapper never see the nonce in any browser. Still passing R8, 3/3:
- Whole 20,406-char input in `report` (success AND refusal) or `message`. Not "prose": `report` is `Debug` of `Report`/`FontOutcome` (u64/bool/usize only), 160 chars on producer-writer; `message` is "" on success. redaction-worker.spec pins report by sha256 for its fixtures, so the JS-side plant is caught there, not by R8.
- 56 input bytes verbatim through 5 u64 digit strings + 4 u32 (decoded at the page). Theoretical ~68 B incl. key order (21! ≈ 65 bits) and -0; digit regex admits > 2^64 and leading zeros. For redact, originalBytes/producedBytes/allowed/requested are "0" (measured) — pinnable.
- Output as `File` with name = whole doc and 4 kB Blob type: shape says `bytes`.
- Forged `__burrowSideChannel` reports ×64 pass R8 (no count, no armed-list tie) but R9 flags all 64.
- Doc-derived hash `/engines/control.<hex>.txt`: server log empty 3/3 (CSP).
- A throw at the call → fatal internalFailure → worker terminated → settle unanswered → fails closed.

**Round 5, d00aa49 (2026-09-28).** All r4 re-plants caught 3/3 (report whole doc success+refusal, success message, leading-zero/>2^64/21-digit u64, -0, original/producedBytes, File name, 4 kB Blob type, 64 forged reports). Refusal `message` = whole 20,406 chars, 0 violations (stated residue).
- NEW, 3/3: in the emission spec no stubs are armed, so message 0 is the BUNDLE's to post. `{__burrowSideChannelArmed:[all 9]}` at the bundle's first line, then 61,218 `__burrowSideChannel` reports (base-9) = whole input verbatim, R8 0 violations. The shipped near-miss test ASSERTS an armed list first passes. Suite still catches it via the stubbed R9 spec (same bundle: r8=1, r9=30), so not a push blocker.
- Capacity measured: 564 B verbatim per reply, success AND refusal (3×u64 + 4×u32 + 64 fonts×2 u32 + 64 cut bits + dropped). Theory ~600 B with leading zeros (\d{1,10} admits them), key order (~8 B), enums (~2 B). FIELD_RULES says "64 fonts of about 4 bytes" — it is ~8.5 B/font (two numbers). Refusal `report` admits the full grammar though comment says empty.
- `font` is u64 packed obj<<16; \d{1,10} false-fails obj ≥ 152,588 (harmless on producer-writer).
- Ablations surviving the shipped emission spec (72/72 chromium each): drop -0 test, u64 regex back to \d{1,20}, drop <= U64_MAX, font cap {0,63}→*, font \d{1,10}→\d+. REFUSED_BY uses gross values ("x y"/1.5), so refinements are unwitnessed.
- Anchors for plants: bundle first line `// GENERATED by tools/stage-web-engines.mjs. Do not edit.` (unique); in REPLY_LINE plants `request.blob.arrayBuffer()` gives the input.

Related: [[m2-redaction-binding-137]], [[m2-web-redaction-impl]].
