# Third-party notices

burrow is distributed under `MIT OR Apache-2.0`. It also incorporates third-party
software, listed here with its license. Every entry must be on the allowlist in
[`deny.toml`](deny.toml) — see [ADR 0003](docs/adr/0003-permissive-licensing.md).

This file is maintained by the `license-auditor` agent and must be updated in the same
pull request that adds or removes a dependency. It is a distribution requirement, not
bookkeeping: the MIT, BSD, and Apache-2.0 licenses all require that their notices travel
with the binary.

Regenerate the Rust portion with:

```bash
cargo deny list --layout crate --format json > /tmp/deny-list.json
```

## Rust dependencies

Only build-time and transitive dependencies so far; no runtime third-party code ships in
the current (M0) build beyond the Rust standard library.

| Crate | License | Used by | Notes |
|---|---|---|---|
| `thiserror` | MIT OR Apache-2.0 | `burrow-types` | Derive macro for error enums; compile-time only |
| `thiserror-impl` | MIT OR Apache-2.0 | `thiserror` | Proc-macro implementation |
| `syn`, `quote`, `proc-macro2` | MIT OR Apache-2.0 | `thiserror-impl` | Proc-macro support; compile-time only |
| `unicode-ident` | (MIT OR Apache-2.0) AND Unicode-3.0 | `proc-macro2` | Unicode identifier tables |

The Rust standard library is distributed under `MIT OR Apache-2.0`.

## Native engines

None linked yet. Planned for M1 onward, recorded in
[ADR 0004](docs/adr/0004-native-engines.md). Each must be license-verified before it is
vendored, and listed here with the exact version and source:

| Engine | Expected license | Status |
|---|---|---|
| PDFium | BSD-3-Clause (plus Apache-2.0 / MIT third-party components) | not yet vendored |
| qpdf | Apache-2.0 | not yet vendored |
| HarfBuzz (`hb-subset`) | MIT (Old Style) | not yet vendored |
| mozjpeg | BSD-3-Clause / IJG | not yet vendored |
| libwebp | BSD-3-Clause | not yet vendored |
| libavif | BSD-2-Clause | not yet vendored |
| jbig2enc | Apache-2.0 — **verify, and verify Leptonica (BSD-2-Clause) too** | not yet vendored |

## npm dependencies

None yet; the web app is scaffolded in this milestone. Astro, Svelte, and Vite are all
MIT.

## Fonts

None bundled yet. Any bundled font must be OFL-1.1 or a permissive alternative, with its
full license text included alongside the font file.
