# burrow

Free, open-source, privacy-first file tools — PDF, image, and video — that run entirely
on your device.

Your files never leave your machine. There is no upload, no account, no server-side
processing, and no telemetry on file content. On the web, the work happens in your
browser tab via WebAssembly; on mobile, in the app itself.

## Status: pre-alpha (M1 in progress)

**There are no working tools yet.** Project setup is done — the Rust workspace, CI,
licensing enforcement, and documentation are in place, the crates compile, and the tests
pass. But no PDF, image, or video operation is implemented.

Nothing here is ready to use for real work. There is no release, no published package,
and no hosted website.

Roadmap, with the current milestone in bold — see [`docs/ROADMAP.md`](docs/ROADMAP.md):

| Milestone | Scope | State |
|---|---|---|
| M0 | Project setup: workspace, CI, licensing, docs | complete |
| **M1** | Merge, split, rotate, reorder, compress — core + web | **in progress** |
| M2 | Redaction, with automatic verification | not started |
| M3 | Android app (Jetpack Compose) | not started |
| M4 | iOS app (SwiftUI) | not started |
| M5 | Office → PDF | not started |
| M6 | PDF → DOCX | not started |

## How it is built

One shared Rust core, three native frontends. No WebView wrappers.

```
core/       Rust: typed errors and limits, engine seams, operations, public API
bindings/   uniffi (Swift + Kotlin) and wasm-bindgen (web)
apps/       web (Astro + Svelte), android (Compose), ios (SwiftUI)
```

The core is deliberately boring and defensive: every input is treated as hostile, every
operation enforces memory/time/page limits, every parser entry point gets a fuzz target,
and no panic is allowed to cross a language boundary. See
[`CLAUDE.md`](CLAUDE.md) for the engineering rules and
[`docs/adr/`](docs/adr/) for why things are the way they are.

## Licensing

burrow itself is dual-licensed under [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option. There is no commercial license, no
dual-licensing upsell, and no paid tier — now or later.

Every dependency must be permissively licensed (MIT, BSD-2/3, Apache-2.0, ISC, Zlib,
MPL-2.0, OFL for fonts, and a short list of additional licenses carried by the bundled
PDF engines). GPL, LGPL, AGPL, SSPL, non-commercial, and unclear licenses are rejected,
and CI enforces this on every commit in two halves: [`cargo-deny`](deny.toml) for Rust
crates, and [`engines/licenses.toml`](engines/licenses.toml) for the native engines and
everything they bundle. See
[ADR 0008](docs/adr/0008-widened-licence-allowlist.md).

## Contributing

Contributions are welcome. Start with [`CONTRIBUTING.md`](CONTRIBUTING.md); note that
tests are considered part of a feature, not a follow-up. Please also read the
[Code of Conduct](CODE_OF_CONDUCT.md).

To report a security vulnerability, do **not** open a public issue — see
[`SECURITY.md`](SECURITY.md).
