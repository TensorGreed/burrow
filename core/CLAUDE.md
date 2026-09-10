# core

The Rust core: every operation, implemented once, for all three platforms. See
[ADR 0002](../docs/adr/0002-rust-core-and-bindings.md).

## Commands

The workspace root is the **repository** root, not this directory, so run these from the
repository root (or pass `--manifest-path`):

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                      # --check to verify
cargo deny check                     # licenses, advisories, bans, sources
cargo audit

# a single crate
cargo test -p burrow-types

# the wasm target CI checks
cargo build -p burrow-wasm --target wasm32-unknown-unknown

# fuzzing lives in its own workspace and needs nightly
cargo +nightly fuzz run <target> -- -max_total_time=60
```

## Crate boundaries

The split exists so the rules are auditable in one place each. Respect the direction of
dependencies; a shortcut across it defeats the point.

```
burrow-types  ← no dependencies of substance, no I/O, no engines
     ↑
burrow-engines ← the ONLY core crate that wraps C/C++ and the only one allowed `unsafe`
     ↑
burrow-ops    ← operations, written against engine traits, never against a C API
     ↑
burrow-core   ← the public surface; the ONLY crate the bindings may depend on
```

- **`burrow-types`** — `Error`, `Limits`. Bindings depend on this directly, so it stays
  tiny and dependency-light.
- **`burrow-engines`** — trait seams over PDFium, qpdf, HarfBuzz, and the codecs. Map
  every engine error code to a typed `Error` here; a raw code must never escape into
  `burrow-ops`.
- **`burrow-ops`** — one module per operation. Takes and enforces `Limits`.
- **`burrow-core`** — re-exports and the stable API. Adding to this surface means the
  bindings need updating too.

## Rules specific to the core

**No network. At all.** Nothing in `core/` may make a network call, or take a dependency
capable of one. This is the project's entire guarantee, and this is where it is kept.

**Nothing panics.** The workspace lints deny `unwrap`, `expect`, `panic!`, `todo!`,
`unimplemented!`, and panicking indexing in library code. They are allowed under
`#[cfg(test)]` via the `cfg_attr` at each crate root. Do not silence a lint at a call
site — restructure the code. If you genuinely need a panic-free unwrap, handle the `None`
case with a typed error.

**Every operation takes `Limits` and enforces them.** Use `Limits::check` so the error
names the limit and both numbers. An unbounded loop or allocation is a denial-of-service
bug, not a missing nicety.

**Casts are denied.** `cast_possible_truncation`, `cast_sign_loss`, and
`cast_possible_wrap` are `deny`: silent numeric truncation on an attacker-controlled size
is a real bug class. Use `try_into()` and map the failure to `Error::Malformed`.

**`unsafe` only in `burrow-engines`.** Every block carries a `// SAFETY:` comment stating
the invariant it relies on and why it holds. Other crates are `#![forbid(unsafe_code)]`.

**Never log or embed file content** in an error, a debug print, or a panic message. Errors
describe the failure, not the input.

## Tests

Four kinds, and an operation is not done without all of them:

| Kind | Where | Asserts |
|---|---|---|
| Unit | `#[cfg(test)]` beside the code | Happy path and each error variant |
| Property | `tests/` with proptest | The operation's invariants (see docs/ROADMAP.md) |
| Golden | `tests/` + committed fixtures | Byte-level expected output |
| Fuzz | `../fuzz/fuzz_targets/` | No panic, hang, or limit escape on any input |

Golden fixtures are committed and small. Large corpora go in `corpus/` and are fetched,
never committed — see `corpus/manifest.toml`.

## Conventions

- New public items need rustdoc, including an `# Errors` section on anything fallible.
  `missing_docs` is `warn` and CI treats warnings as errors.
- Error enums are `#[non_exhaustive]`; adding a variant should not be a breaking change.
- Operations are named as verbs (`merge`, `split`, `rotate`).
- Commits use the `core` scope, or the crate: `feat(core): add page reordering`.
