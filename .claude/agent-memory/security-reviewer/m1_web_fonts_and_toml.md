---
name: m1-web-fonts-and-toml
description: Measured facts from reviewing M1 PR A (design tokens, vendored woff2, shared tools/toml.mjs) — how to audit the font binary offline, what the e2e font timing really rests on, and the toml.mjs prototype-pollution repro.
metadata:
  type: project
---

Reviewed 2026-09-12 on branch `feat/web-design-tokens`. Facts that cost real digging.

**A vendored woff2 can be audited offline, with node alone.** No fontTools on this machine,
and `python3 -c 'import fontTools'` fails. `node:zlib`'s `brotliDecompressSync` plus a ~40-line
woff2 header/table-directory walk (UIntBase128 lengths, known-tag table, tables laid end to end
in the decompressed stream, only glyf/loca transformed) gets you the raw `name`, `cmap` and
`fvar` tables. Script kept at `/tmp/.../scratchpad/woff2.mjs` pattern — cheap to rewrite.

Measured for `apps/web/public/fonts/atkinson-hyperlegible-next-v2.001-latin.woff2`
(sha256 `12e31991…f026c3`, 18,548 bytes, matches `apps/web/fonts.toml`): 20 tables, name IDs
0/13/14 present and exactly the OFL strings, copyright line carries **no** Reserved Font Name,
family "Atkinson Hyperlegible Next", version 2.001, `wght` axis min/default/max 400/400/700,
207 cmap codepoints — every claim the manifest makes about the *shipped bytes* is true.
**woff2 `metaLength` and `privLength` are both 0**; those two fields are an arbitrary-bytes
channel in the container and are worth checking on any future font.

**What is not verified, and cannot be offline:** nothing derives the shipped file from
`source_sha256` / `source_commit` / `build_commands`. Unlike `engines/fetch.sh`, no script
fetches or re-digests the upstream TTF. The machine check is only "these are the bytes someone
recorded".

**The font is safe for `e2e/zero-requests.spec.ts` because the suite is serial.** Measured from
`apps/web/test-results/request-log.jsonl` (one full run): exactly 90 document loads, 90 font
requests, 90 CSS requests — **one font fetch per page load in all three browsers**, so the
`crossorigin` attribute really is preventing the duplicate uncredited fetch. The font always
lands during `openHarness`'s page load, before `mark()`. That holds only because
`playwright.config.ts` sets `workers: 1` / `fullyParallel: false`: every spec appends to one
shared log, and a post-hoc scan of the final log shows `/harness`, `/_astro/*.css`,
`/host/*.js` and `/favicon.ico` from later specs sitting inside earlier markers' windows. The
font adds no new dependency on that — but raising `workers` breaks the whole file, not just the
font. **Do not add `/fonts/` to `isPinnedArtifact`**; that would license a post-init fetch.

**`tools/toml.mjs` has a prototype-pollution primitive.** Reported, may or may not be fixed:

```
[__proto__]
polluted = "yes"
```

sets `current = Object.prototype` (`root["__proto__"] ??= {}` is a no-op because it is not
nullish), and every following `key = value` writes onto `Object.prototype` for the rest of the
process — verified: `({}).polluted === "yes"`. `[[__proto__]]` instead throws
`TypeError: root[key].push is not a function`. tomllib sees an ordinary table named
`__proto__`, so `credits.test.ts`'s "the two readers agree" assertion does not cover this
class; the module's own docstring claim that the two readers agree is not true for it.

**Astro inlines no stylesheet only because of `vite.build.assetsInlineLimit: 0`.** Astro's
`build.inlineStylesheets: 'auto'` uses that number as its threshold. Measured: 2,312- and
3,215-byte CSS chunks emitted as external `<link>`, both under the 4 kB default. Raising that
vite option would inline `<style>` into every page, which `style-src 'self'` (no
`'unsafe-inline'`, no nonce) refuses.

Related: [[m1-web-engine-path]], [[m1-ci-check-vacuity]], [[review-in-progress-edits]].
