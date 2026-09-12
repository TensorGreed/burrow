---
name: add-operation
description: Checklist for adding a file operation to burrow end to end — core implementation, typed errors, limits, all four test kinds, fuzz target, bindings, web UI, and docs. Use when adding or substantially changing an operation like merge, split, rotate, or redact.
---

# Adding an operation

An operation is one vertical slice. It is **not** done when the core function works — it
is done when every layer below is present. Skipping a layer leaves a gap that is much
harder to close later, and in this project the gaps are security bugs.

Read `CLAUDE.md` and `core/CLAUDE.md` before starting. Check `docs/ROADMAP.md` for the
operation's stated invariants; they are what the property tests assert.

Work through this in order. Later steps depend on earlier ones being right.

## 1. Design the API before writing it

- What is the narrowest input and output that expresses the operation? It has to cross
  two FFI boundaries, so favour plain values over rich types.
- What can go wrong, and which of those are distinct errors a caller would handle
  differently? That set becomes the error variants.
- Which `Limits` apply, and what does the operation do when one is hit?
- Which engine does the work — PDFium, qpdf, or both? A file PDFium rejects may be
  repairable by qpdf first.

If any of this is genuinely open, write or update an ADR rather than deciding it in a
function signature.

## 2. Core implementation — `core/burrow-ops/`

- One module per operation. Take `&Limits` and enforce it.
- Write against the engine traits in `burrow-engines`, never a C API directly.
- Validate every length, offset, and count read from the file **before** using it to
  allocate, index, or seek.
- No `unwrap`, `expect`, `panic!`, or panicking indexing. No `as` casts on
  file-controlled numbers — use `try_into()` and map failure to `Error::Malformed`.
- Never put file content in an error message, log, or debug output.
- rustdoc on every public item, including an `# Errors` section.

## 3. Typed errors — `core/burrow-types/src/error.rs`

- Add variants for failure modes that are genuinely new. Reuse existing variants where
  they fit; a taxonomy nobody can hold in their head is not useful.
- `Error` is `#[non_exhaustive]`, so adding a variant is not a breaking change.
- Map engine error codes to variants **inside `burrow-engines`**. A raw code must never
  escape that crate.

## 4. Limits

- Use `Limits::check(name, requested, allowed)` so the error names the limit and both
  numbers.
- Check **before** allocating, not after.
- Consider aggregate cost, not just per-item: a thousand small pages can exceed what one
  large page would. Watch for amplification — page count times page size, multiplied
  pixel dimensions, fonts re-embedded per page.
- Make cancellation actually stop work, not just stop reporting it.

## 5. Tests — all four kinds

Delegate to the `test-engineer` agent, or follow the same standard. See
`core/CLAUDE.md` for the table.

- **Unit** — happy path and *every* error variant. `assert!(result.is_ok())` alone is not
  coverage. `Error` has no `PartialEq`; use `matches!`.
- **Property** (proptest) — the invariants from `docs/ROADMAP.md`. Keep any minimised
  failing case as a unit test.
- **Golden** — committed expected output, small fixtures. Large corpora go in `corpus/`
  and are fetched, never committed.
- **Limits** — an oversized input returns `LimitExceeded`, not a crash or an OOM. Test
  the boundary and one past it.

### If the operation brings its own CHECK, it ships with probes

An operation that adds a validator, a detector, a linter or any rule-driven gate — anything
with a list of patterns, a symbol sweep, a name allowlist — must give each rule **its own
positive fixture and a near-miss**, verified on **every run** rather than only when a
self-test happens to run.

This is not a style preference. Every rule-driven check in this repository has, at some
point, contained a rule that matched nothing while the output reported a healthy count:

| | |
|---|---|
| `check-no-generated-files.sh` | 15 of 16 patterns truncated to `(^` and inert; output said "16 pattern(s)" |
| `detect-engine-components.py` | the ICU fingerprint rejected `ubidi_setPara_78` — the example in its own comment |
| `check-engine-licences.py` | 4 of 15 licence texts compared in CI; identical output to all 15 |

**A rule that matches nothing passes everything. A rule that matches everything fails
everything.** Neither is a check, and neither is visible from a green run.

So, per `CLAUDE.md`'s definition of done:

- each rule matches its own fixture and rejects a near-miss, asserted on every invocation;
- the check reports what it examined, and gates on the expected count where that count is
  derivable — `git ls-files`, a manifest, a workspace member list — not merely on non-zero;
- the probe gate itself has a test: break a rule in a **copy** of the checker and assert it
  refuses, **naming the reason**. Keep the copy beside the original; one in a temp directory
  resolves its own paths wrongly and exits non-zero for the wrong reason, which an
  exit-code-only assertion reports as a pass.

## 6. Fuzz target — `fuzz/fuzz_targets/`

Required for **every new parser entry point**. Not optional, not deferred.

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Errors are expected. Panics, hangs, and OOMs are bugs.
    let _ = burrow_core::ops::<operation>(data, &burrow_core::Limits::default());
});
```

Register it in `fuzz/Cargo.toml` and run it clean for at least 60 seconds before opening
a pull request:

```bash
cargo +nightly fuzz run <target> -- -max_total_time=60
```

`fuzz/` is a separate workspace and needs nightly — see `fuzz/README.md`.

## 7. Bindings

- **`bindings/burrow-wasm/`** — expose the operation via wasm-bindgen. It runs in a Web
  Worker; report progress and support cancellation.
- **`bindings/burrow-ffi/`** — expose it via uniffi (from M3). Route every entry point
  through `guard` so no panic can cross into Swift or Kotlin.
- Bindings marshal types and nothing else. No logic, no validation, no limit enforcement
  — there must be exactly one implementation of every rule.
- If you deliberately defer a binding, say so in the pull request. Do not leave it
  silently missing.

## 8. Web UI — `apps/web/`

Read `apps/web/CLAUDE.md`.

- Its own route at its own indexable URL, server-rendered, with a real title,
  description, and prose. Users arrive from search.
- Load the wasm module lazily, when the user engages with the tool — never on page load.
- Run the operation in a Web Worker, with progress and cancel.
- No third-party requests. Self-host every asset.
- Report the typed error from the core; never echo file content.
- Keyboard usable, and the drop zone has a file-input fallback.
- A Playwright test drives it with a real file, and the no-network assertion still passes.

## 9. Docs

- rustdoc with an `# Errors` section on everything fallible.
- Tick the operation off in `docs/ROADMAP.md`.
- An ADR if you made a decision worth asking about twice.
- `THIRD_PARTY_NOTICES.md` if you added a dependency — and run the `add-dependency`
  checklist for it.

## 10. Verify, then get it reviewed

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo build -p burrow-wasm --target wasm32-unknown-unknown
pnpm -C apps/web lint && pnpm -C apps/web check && pnpm -C apps/web build && pnpm -C apps/web test
cargo +nightly fuzz run <target> -- -max_total_time=60
```

Then run the `code-reviewer` and `security-reviewer` agents. For anything touching
redaction, the `security-reviewer` pass is mandatory and is the highest-priority review
in the project.

Finally, walk the [definition of done](../../../CLAUDE.md#definition-of-done) yourself.

## Commit

Conventional Commits, scoped to the area: `feat(core): add page reordering`,
`feat(web): add reorder tool page`. Split into reviewable commits that each pass CI where
you can.
