---
name: m1-differential-harness
description: Measured properties of the M1 PR 4b differential conformance harness — what the freshness check really binds, and what the committed bombs actually cost (2026-09-11)
metadata:
  type: project
---

M1 PR 4b (`feat/differential-conformance`, d9ef384) added `tests/conformance/expectations.json`
schema 2, `apps/web/src/conformance/compare.ts`, and adversarial fixtures. Measured on
2026-09-11, aarch64, native engines from `engines/vendor/native-aarch64`:

**Cost of the committed bombs (native, debug test binary).**
- Whole 21-case × 2-operation corpus run: **1.57 s, 500 MB peak RSS**.
- `objstm-bomb.pdf` (204 kB, inflates 1026× to exactly 200 MiB): PDFium RSS delta
  **207,044 kB (202 MiB) retained while the handle is open**, process peak ~420 MB. It is
  retained, not transient, so the `objstm-bomb-tight-ceiling` expectation
  (`measured`, 96 MiB + 64 MiB noise margin = 160 MiB threshold, observed `requested`
  210,739,200) has ~41 MiB of margin and is *not* allocator-noise flaky.
- The whole suite **SIGABRTs silently (core dumped, exit 134, no message)** under
  `ulimit -v 450000` — PDFium's own OOM abort, which no `catch_unwind` intercepts. So the
  corpus needs ~500 MB of address space to run at all.
- The three `bomb-hidden-*` pre-scan-bypass fixtures are now refused at `Stage::Prescan` in
  0.01 s / 128 kB by **both** engines, i.e. the PR 3 bypasses in
  [[m1-prescan-key-scan-bypass]] are fixed and the fixtures are cheap to run routinely.

**`compare.ts` freshness binds records to each other, not to the corpus.** The loop
`for (const record of [native, web]) if (record.expectations_sha256 !== web.expectations_sha256)`
is trivially true for `web`, so it only asserts native === web. Verified by driving `compare`
directly: two records carrying the *same wrong* digest pass clean, and
`compare(exp, webRecord, webRecord, [])` also passes clean with a full comparison count —
`record.platform` is never checked. It is safe today only because
`e2e/conformance.spec.ts` computes `expectationsDigest` itself from the bytes it parsed; the
comparator has no independent tie and its unit tests cannot catch a caller regression.

**`parseAllowlist` fails closed except for two things** (also driven directly): a duplicate key
inside one `[[divergence]]` block silently takes the **last** value (real TOML errors), and the
greedy `"(.*)"` regex swallows trailing junk after the closing quote, so
`issue = "https://x/1" # todo"` still passes the `https://` check.

**How to apply:** when reviewing later changes to this harness, re-run the two direct-drive
probes (self-compare and both-stale) before believing the freshness claim, and re-measure the
objstm bomb rather than trusting the 200 MiB figure — the expectation is an inequality against
an RSS delta, and a PDFium bump moves it. See [[m1-limits-real-strength]] and [[user-role]].
