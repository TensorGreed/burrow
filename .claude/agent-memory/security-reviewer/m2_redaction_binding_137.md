---
name: m2-redaction-binding-137
description: #137 binding half (6f577de) — the harness-only hold is sound on the deploy path but caught there only by accident; untested report counts; which redact-main.js guards survive the e2e; e2e port collisions with a parallel reviewer.
metadata:
  type: project
---

Reviewed 2026-09-26 at 6f577de (branch feat/137-redaction-binding). No exploitable leak found.

- **The hold.** `stage-web-engines.mjs` keys the third bundle on `BURROW_HARNESS === "1"`, the same test `astro.config.mjs` uses, and `pnpm build` runs prebuild in the same env, so the deploy cannot stage it without also shipping /harness (which check-deployable-build refuses). Mutation that stages it in production dies in vitest `production-build.test.ts` (held-op scan + one-bundle rule). **Off the CI path:** harness staging then a bare `astro build` gives a dist that passes check-deployable-build AND check-redaction-not-in-base (excludes `burrow_wasm_redact_*` by name). Only check-pdfium-is-render-only.sh refuses it, via its "exactly 4 wasm modules" count, so that count is now load-bearing for the hold without saying so.
- **Untested:** `Reply::redacted`'s `retained_fonts` / `dropped_carried_text` (nothing reads them in any test; the §7 disclosure will). The e2e cases all have report digests with no retained font.
- **redact-main.js e2e survivors (chromium):** region typeof check, covered validation, the check_input_budget verdict, and R8/R9 plants (an id-less chunk post, a blob: URL before the call). Killed: the isInteger drop, the left/top swap, a chunk posted with the request id.
- redaction.rs doc says check-pdfium-is-render-only.sh holds "no PDFium in the redact module"; that script refuses harness builds and the bundle exists only in them. Measured: 52 imports, 0 PDFium.
- **Method:** e2e ports 4391/4392 collided with the code reviewer's run (EADDRINUSE; one run lost). Use an unusual port pair such as 4471/4472 and check `ss -ltnp` first.

**Round 2 (96d4689):** every survivor above except the R8/R9 plants now dies (region, covered and count swap by e2e; budget by vitest input-budget.test.ts only, since the e2e cannot tell the pre-check from the engine's own max_input_bytes). The by-name hold refusal is in check-redaction-not-in-base.sh, which is NOT in deploy.yml, so on deploy the pdfium module count still holds the line alone. The redaction e2e needs native qpdf plus `engines/vendor/native-aarch64` (cjpeg) to generate fixtures, and it never removes its mkdtemp dir.

Related: [[m2-web-redaction-impl]], [[m1-render-worker-bundle]], [[m2-redact-verify]].
