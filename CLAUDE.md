# burrow

Free, open-source, privacy-first file tools (PDF, image, video) that run entirely on the
user's device. One shared Rust core; three frontends (web, Android, iOS).
Licensed `MIT OR Apache-2.0`. No commercial licensing, ever.

## Non-negotiables

These are not preferences. A change that violates one is wrong, however well it works.

1. **Privacy.** File content never leaves the device. No code path that touches user file
   content may make a network call. No telemetry on file content, ever.
2. **Permissive licenses only.** Allowed: MIT, BSD-2/3, Apache-2.0, ISC, Zlib, MPL-2.0,
   OFL (fonts). Forbidden: GPL, LGPL, AGPL, SSPL, non-commercial, and anything unclear.
   CI enforces this via `cargo-deny`. See `docs/adr/0003-permissive-licensing.md`.
3. **All input is hostile.** Every file is untrusted and possibly adversarial. Every
   parser entry point gets a fuzz target. No panics cross the FFI boundary; errors are
   typed. Every operation enforces memory, time, and page/pixel limits.
4. **Correctness before features.** Most of all in redaction, where a bug leaks secrets.
   Redaction output is verified automatically after every run.
5. **Tests are part of the feature.** Every operation ships unit, property (proptest),
   golden-file, and fuzz tests. CI must be green to merge.

## Commands

Rust (the cargo workspace root is the repository root; run these from there):

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                 # cargo fmt --all -- --check to verify
cargo deny check                # licenses, advisories, bans
cargo audit
cargo build -p burrow-wasm --target wasm32-unknown-unknown
cargo doc --workspace --no-deps   # RUSTDOCFLAGS="-D warnings" in CI
```

Web (`apps/web/`):

```bash
pnpm install
pnpm dev            # local dev server
pnpm build          # static output to dist/
pnpm check          # astro check + svelte-check
pnpm lint           # prettier --check
pnpm test           # vitest
```

Fuzzing (`fuzz/`, its own workspace, nightly toolchain):

```bash
cargo +nightly fuzz list
cargo +nightly fuzz run <target> -- -max_total_time=60
```

## Where things live

| Path | Contents |
|---|---|
| `core/burrow-types/` | Typed errors, `Limits`, shared value types. No I/O, no engines. |
| `core/burrow-engines/` | Engine trait seams and FFI wrappers (PDFium, qpdf, …). |
| `core/burrow-ops/` | Operations (merge, split, redact, …) built on engines. |
| `core/burrow-core/` | Public API surface. The only crate bindings depend on. |
| `bindings/burrow-ffi/` | uniffi glue for Swift + Kotlin. |
| `bindings/burrow-wasm/` | wasm-bindgen glue for the web. |
| `apps/web/` | Astro + Svelte islands. One indexable page per tool. |
| `apps/android/`, `apps/ios/` | Native Compose / SwiftUI apps (M3 / M4). No WebViews. |
| `corpus/` | `manifest.toml` is tracked; `corpus/files/` is gitignored. |
| `tools/` | Corpus runner, visual diff, headless scripts. |
| `fuzz/` | cargo-fuzz targets, one per parser entry point. |
| `docs/adr/` | Architecture decision records. Add one for any decision worth asking twice. |

## Conventions

**Errors.** Every fallible function returns `Result<T, E>` with a concrete `thiserror`
enum. Error enums are `#[non_exhaustive]`. Never `unwrap()`, `expect()`, `panic!()`,
`todo!()`, `unimplemented!()`, or indexing that can panic in library code — tests and
`build.rs` may. Convert engine error codes into typed variants at the engine boundary;
do not let raw codes escape into `burrow-ops`.

**Panics and FFI.** No panic may cross an FFI boundary. Wrap entry points in
`catch_unwind` where a panic is conceivably reachable, and map it to an
`Error::Internal` variant. `panic = "abort"` is not an acceptable substitute.

**Limits.** Every operation takes a `Limits` and enforces it. A missing limit is a
denial-of-service bug, not a nicety.

**`unsafe`.** Crates default to `#![forbid(unsafe_code)]`. Only `burrow-engines` and
`burrow-ffi` may relax it, to `#![deny(unsafe_op_in_unsafe_fn)]`. Every `unsafe` block
carries a `// SAFETY:` comment stating the invariant it relies on and why it holds.

**Naming.** Crates and directories `burrow-kebab-case`; Rust items `snake_case` /
`CamelCase`; features `kebab-case`. Operations are verbs (`merge`, `split`, `rotate`).
Avoid abbreviations except the established ones (`pdf`, `dpi`, `ffi`).

**Commits.** [Conventional Commits](https://www.conventionalcommits.org/):
`feat|fix|docs|test|refactor|perf|build|ci|chore(scope): subject`. Imperative,
lowercase, no trailing period. Scope is the crate or app (`core`, `web`, `ci`).
Breaking changes get a `!` and a `BREAKING CHANGE:` footer.

**Dependencies.** Adding one is a decision, not a detail. Follow the `add-dependency`
skill and get a `license-auditor` pass before merging.

## Definition of done

A change is done when all of these hold:

- [ ] `cargo fmt --all -- --check`, `cargo clippy … -D warnings`, `cargo test --workspace` pass.
- [ ] `cargo deny check` passes; any new dependency has a `license-auditor` pass and a
      `THIRD_PARTY_NOTICES.md` entry.
- [ ] New parser entry points have a fuzz target that runs clean for 60s.
- [ ] New operations have unit, property, golden, and fuzz tests, and enforce `Limits`.
- [ ] No new `unwrap`/`expect`/`panic!` in library code; new `unsafe` has `// SAFETY:`.
- [ ] No new network call reachable from code that touches file content.
- [ ] Public API changes are reflected in bindings (uniffi + wasm) or explicitly deferred.
- [ ] Docs updated: rustdoc on public items, plus `docs/ROADMAP.md` or an ADR if scope
      or a decision changed.
- [ ] Commit messages follow Conventional Commits; CI is green.

## Working agreements

- Ask rather than assume. If two readings of a task lead to materially different work,
  raise it with a recommendation and the tradeoffs.
- Prefer `rg` over `grep`, and the file tools over shell text editing.
- Do not add docs, changelogs, coverage passes, or formatting sweeps that were not asked for.
- Do not commit or push unless asked.
- Report faithfully. If tests fail, say so and show the output. Never claim a step passed
  without running it.
