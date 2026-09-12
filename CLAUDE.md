# burrow

Free, open-source, privacy-first file tools (PDF, image, video) that run entirely on the
user's device. One shared Rust core; three frontends (web, Android, iOS).
Licensed `MIT OR Apache-2.0`. No commercial licensing, ever.

## Non-negotiables

These are not preferences. A change that violates one is wrong, however well it works.

1. **Privacy.** File content never leaves the device. No code path that touches user file
   content may make a network call. No telemetry on file content, ever.
2. **Permissive licenses only.** Allowed: MIT, BSD-2/3, Apache-2.0, ISC, Zlib, MPL-2.0,
   OFL (fonts), Unicode-3.0, CC0-1.0, Unlicense — plus, for bundled native engine
   components, FTL, IJG, libpng-2.0, LicenseRef-AGG-2.3, MIT-Modern-Variant and ICU —
   plus NCSA, for libFuzzer (ADR 0012).
   Forbidden: GPL, LGPL, AGPL,
   SSPL, non-commercial, and anything unclear. CI enforces this in two halves:
   `cargo-deny` for Rust crates, `tools/check-engine-licences.py` against
   `engines/licenses.toml` for the engines. Adding a licence needs an ADR, not a config
   edit. See `docs/adr/0008-widened-licence-allowlist.md`, `docs/adr/0010-harfbuzz-and-icu-in-pdfium.md`
   and `docs/adr/0012-ncsa-for-libfuzzer.md`.
3. **All input is hostile.** Every file is untrusted and possibly adversarial. Every
   parser entry point gets a fuzz target. No panics cross the FFI boundary; errors are
   typed. Every operation takes a `Limits` and applies every ceiling in it. Input size, page
   count and pixel count are checked exactly, before anything is allocated. Time is
   cooperative and memory is **detected, not bounded** — see *Limits* below.
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
cargo audit                     # NOT optional when replicating CI -- see below
cargo build -p burrow-wasm --target wasm32-unknown-unknown
cargo doc --workspace --no-deps   # RUSTDOCFLAGS="-D warnings" in CI
```

Native engines (needed for `--all-features`; see `docs/adr/0004-native-engines.md`):

```bash
engines/fetch.sh              # pinned + checksum-verified engine artifacts
engines/build-native.sh       # zlib, libjpeg-turbo, qpdf (+ an ASan/fuzzer variant)
engines/build-wasm.sh         # the same engines for wasm
python3 tools/check-engine-licences.py   # engines/licenses.toml vs ADR 0008

cargo test --workspace --all-features    # links the engines; without --all-features it does not
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
| `tests/conformance/` | Small committed fixtures plus `expectations.json`, the typed outcome each must produce. Shared by the native tests and (from PR 4) the web differential harness, so it belongs to neither crate. |
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

**Limits.** Every operation takes a `Limits` and applies every ceiling in it. A missing limit
is a denial-of-service bug, not a nicety.

**They are not equally strong, and the difference is documented rather than smoothed over.**
`max_input_bytes`, `max_pages` and `max_pixels` are exact. `max_duration_ms` is cooperative:
overshoot of up to one engine call is possible. **`max_memory_bytes` bounds nothing on any
platform** — a structural pre-scan and a length-based estimate before the engine, a measured
check after it, so an overrun is *detected*, not prevented. Say "detect" where we detect and
"bound" only where something is actually bounded; `burrow_types::Limits`' rustdoc and
`docs/adr/0007`'s 2026-09-12 amendment carry the per-path detail. An overclaiming doc comment
here is a bug, not a wording preference: this one was load-bearing in two later ADRs' reasoning
before PR 4b measured it.

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

**Reviews run before the first push, not after.** `security-reviewer` and `code-reviewer` go
over the change while it is still local. In M1 PR 4a-ii they ran after the branch was pushed
and found two things that had already reached a commit: a reachable bug that took a page
offline after three long operations, and a regression that silently disabled two CSP tests by
turning an assertion into a tautology. Both would have been caught before anyone else could
pull them. A review that happens after the push is a review of history.

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
- **A CI monitor watches one run ID, and is stopped on every push.** Use
  `gh run watch <id> --exit-status`, never `gh pr checks <pr>`: the latter reports whichever
  run is *current*, so a monitor started for one commit silently begins reporting on another.
  Four of them accumulated across one PR, all polling the same endpoint — a green from any
  would have read as confirmation while saying nothing about the commit it was started for.
  Stop the previous monitor before starting the next.
- **`gh run watch --exit-status` is not the verdict. Confirm with
  `gh run view <id> --json conclusion`.** Measured in M1 PR 4c: the watcher exited **0** on a
  run whose conclusion was `failure`. Had that exit code been trusted, a red run would have
  been reported as green — the precise failure the run-ID rule above exists to prevent, one
  layer further in. Read the run's conclusion, and read the per-job conclusions with
  `--json jobs` when you need to know *which* job failed and whether it is yours.
- **Replicating CI locally means every job, and `cargo audit` is the one that gets skipped.**
  It is in the command list above and it is easy to run twelve checks without it, because it
  is the only Rust gate that consults something outside the repository. In M1 PR 4c the
  branch was pushed after twelve green local checks and CI went red on the thirteenth. The
  failure was not even ours — the audit *tool* would not build — but the point stands: a
  local sweep that omits a job is not a replication of CI.
- **Running a Python tool by importing it writes a `.pyc`, so never `git add -A` afterwards.**
  Verifying `tools/*.py` by `importlib`-ing it leaves `tools/__pycache__/`, and a blanket add
  commits an opaque binary. That reached `main` once, in PR #29. `.gitignore` covers it now,
  but the ignore rule cannot untrack a file already staged on a branch that predates it, and
  `git rm --cached` is then the only way out. Prefer `tools/check-python-syntax.py`, which
  compiles in memory and writes nothing.
- **Where a check lives in `ci.yml` is load-bearing, and consolidating jobs breaks checks
  silently.** `engines/vendor/` is gitignored and only some jobs fetch it, so a check that
  reads the vendor tree must live in a job that has one, and a check that reads only
  committed files should live in a job that fetches nothing so it still runs on a clean
  checkout. Two measured failures of this kind: the engine-licence *drift* comparison ran
  only in `deny`, which fetches nothing, so it was firing on **zero** components; and the
  qpdf crypto assertion lived inside a build step gated on a cache miss, so on an ordinary
  PR with a warm cache it did not run at all. **A check that silently examines nothing is
  worse than no check** — it reads as coverage. Before moving a step between jobs, ask what
  it reads and whether that will be there.
- Report faithfully. If tests fail, say so and show the output. Never claim a step passed
  without running it.
- **Run `security-reviewer` and `code-reviewer` before the first push.** See *Conventions*;
  this is the working-agreement half of the same rule.
