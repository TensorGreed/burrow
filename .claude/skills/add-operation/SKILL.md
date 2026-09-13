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

## 2a. If the operation needs a capability no engine has yet

**Check before you plan the operation, not while you are writing it.** Every engine capability
in the repo before M1 PR B was read-only — `DocumentEngine` was `{ open, page_count }` and
`StructureEngine` was `{ check }`, and neither bridge could return bytes *out* of an engine
heap. `merge` could not be built inside `burrow-ops` at all until that existed, and the work
crossed **six individually-gated artifacts** that no other step in this checklist mentions:

| | where | the gate |
|---|---|---|
| a new trait method | `core/burrow-engines/src/lib.rs` | the native and web impls are separate on purpose, so the differential corpus can catch them diverging |
| new `unsafe extern "C"` declarations | `…/<engine>/ffi.rs` | every one needs a `// SAFETY:` comment, and for qpdf an entry on `engines/qpdf-trapped-functions.txt` or an argued one in `engines/qpdf-untrapped-accepted.toml` (ADR 0013) |
| a new bridge method | `…/web/bridge.rs`, `apps/web/src/worker/bridge.js`, `…/web/fake.rs` | ADR 0009 §2: the bridge method list **is** the audit surface, and no branch on engine state may live in JS |
| the wasm export list | `engines/build-wasm.sh` | an allowlist mirroring `ffi.rs`. A missing export fails at run time in the browser and nowhere else |
| an engine rebuild | `engines/build-wasm.sh` | new content hashes, a regenerated CSP, and a **size-budget re-measure** |
| the reply shape | `bindings/burrow-wasm/src/lib.rs` | a reply that carries bytes, or an index, or an inner kind, is a shape change every reader has to follow |

Two things that were learnt the hard way and are cheap to avoid:

- **A wrapper error must be unwrapped everywhere its detail is read, not only where its kind
  is read.** `Error::InputFailed` names *which* input and never *what*, so `is_fatal` and the
  reported kind looked through it while the limit name, the stage and both numbers did not —
  and a real ceiling reached the page as a vague sentence. Fix it on **both** sides at once, or
  the corpus records the divergence as agreement.
- **A multi-input or byte-returning operation changes the conformance schema**, not just the
  expectations. **Three readers move together**, and it is easy to find two of them and stop:
  `core/burrow-engines/testsupport/expectations.rs` builds the native outcome,
  `apps/web/e2e/conformance.spec.ts`'s `outcomeOf` builds the web one, and
  `apps/web/src/conformance/compare.ts` renders both for comparison. Expected outcomes stay
  hand-written, never recorded from what the engine currently returns.

## 2b. If the output is a SUBSET of the input, it may carry nothing derived from the rest

**Split, extract, redact — anything that emits part of what it was given.**

> The emitted bytes must contain **no data derived from the excluded content**. Not the content,
> and not anything computed from it: titles, names, destinations, counts, thumbnails, or index
> entries that describe what was taken away.

This is not a style rule and it is not obvious from a green test. `split`'s first implementation
route passed every check anyone would think to write — the right pages were present, the file
opened, the page count was correct — and a two-page output carried the outline titles of all
five source pages, because it kept the source's catalog whole. Somebody splitting pages 2-3 out
of a document to send to another person would have been sending the titles of pages 4 and 5 with
them. [ADR 0019](../../../docs/adr/0019-how-split-builds-its-outputs.md) §2.

**It is the property redaction depends on**, and it is already written down for redaction:
[ADR 0006](../../../docs/adr/0006-wasm-linking-strategy.md)'s R8, R9 and R10 are three
statements about one thing — what matters is the bytes that are **emitted**, not what the
operation meant to emit. R10 in particular requires verification to run on the exact byte
sequence emitted, never on a sibling copy. Split is the same disagreement between intent and
output, arriving without a verifier to catch it.

**Prefer building an output to carving one — and know that building is not sufficient.** Start
from nothing and copy in what belongs, rather than starting from everything and removing what
does not. Carving is a denylist over a structure whose design permits keys nobody enumerated.

But "build" is an allowlist over the **container's keys**, not over **objects**. `split` was
written on that assumption and it was wrong: the engine's copy is a *reachability closure*, so
anything an included item points at comes too — a resource dictionary inherited from a parent
node, a form field whose siblings live on excluded pages, a shared annotation array. ADR 0019
§2a lists six measured channels, including a form field's typed **value** arriving in an output
that does not contain the page it was typed on.

So: build, and then ask separately what the copy dragged along. The two questions are not the
same one.

### The fixture has to make the leak POSSIBLE

The first `split` fixture gave every page its own annotations, its own resources and its own
everything. Under that structure the leak class **cannot occur**, so twenty canaries confirmed
a case that was never at risk and the test passed for a year's worth of confidence in an hour.

A subsetting fixture must therefore contain **shared** structure: an object referenced from both
a kept and an excluded item, a parent node carrying inherited attributes, a container listing
children on both sides of the cut. If no two parts of the fixture share an indirect object, the
fixture cannot fail the test.

**Enumerate by mechanism, not by feature.** "Outline, field, annotation, attachment" is a list
of things you thought of. "Everything reachable from a kept object" is the property. Where you
can, assert the property structurally — plant a unique marker in *every* source object and
assert the output's marker set is a subset of the closure of what was kept — because that
version fails for categories nobody named, and a canary list never will.

### The required test

Not a principle. A test, and one that has been shown to fail:

- **Marker-bearing fixtures where every excluded item is a unique canary.** Each page's outline
  title, field name, annotation text and attachment name names its own page, so "did something
  from page 4 come along" is a search rather than a structural walk.
- **Scan the emitted bytes, decompressed.** `qpdf --qdf --object-streams=disable`. A canary inside a compressed stream
  is invisible to a naive search, and a scan that finds nothing reports the same green whether
  it is working or broken.
- **A control that deliberately leaks.** Run the identical assertion against an implementation
  known to carry excluded data, and require it to FAIL. Without the control, the scan is a claim.
- **Both encodings.** A real outline title or field name is often a UTF-16BE text string, which
  a byte-literal ASCII search will not find. Plant at least one canary per category in UTF-16BE
  and search for both, or the scan is blind to how producers actually write strings.

**What this test can and cannot say.** It confirms the enumeration it was given and nothing
more. `qpdf --qdf` does not decode `/DCTDecode`, `/JPXDecode` or `/JBIG2Decode`, so data that
exists only in transformed form — a font subset still carrying glyphs for removed characters,
an image's pixels — is outside its reach. For redaction that transformed form *is* the leak,
so M2 needs more than this test, and should not inherit it as though it were sufficient.

## 2c. A fidelity fixture that cannot fail the test is not a fixture

Two of these in one milestone, each producing a confident table that measured nothing:

| | what was wrong | what it reported |
|---|---|---|
| `split`'s first fidelity run | the **merge** fixtures are one page each, so a one-page split is the **identity** | both candidate routes perfect |
| `merge`'s first browser order test | every conformance fixture has the same empty page, so **any order produces identical bytes** | order preserved |

So, before trusting a fixture set:

- **Make the operation non-identity.** If the output can equal the input, the fixture cannot
  distinguish an implementation from a no-op. A split fixture needs more pages than the split
  takes; a rotate fixture needs a page that is not square.
- **Make the inputs distinguishable.** A set whose members are interchangeable cannot test
  ORDER or SELECTION — the two things most operations are mostly about. Give each page, or each
  input, something unique and cheap to find: a different `/MediaBox`, a numbered marker.
- **Check that a passing result is not the trivial one.** Ask what the test would report if the
  operation were replaced by "return the input unchanged". If that is a pass, the fixture is the
  problem.

## 3. Typed errors — `core/burrow-types/src/error.rs`

- Add variants for failure modes that are genuinely new. Reuse existing variants where
  they fit; a taxonomy nobody can hold in their head is not useful.
- `Error` is `#[non_exhaustive]`, so adding a variant is not a breaking change.
- Map engine error codes to variants **inside `burrow-engines`**. A raw code must never
  escape that crate.

## 4. Limits

- Use `Limits::check(Stage::…, name, requested, allowed)` so the error names the limit, the
  **stage**, and both numbers. The stage has been the first argument since M1 PR 4b and it is
  not decoration: three different checks produce `LimitExceeded { limit: "max_memory_bytes" }`,
  and without the stage a rejection can move between the pre-scan and the measured check with
  nothing noticing.
- **Decide whether a ceiling applies per input or to the total, and say which in the rustdoc.**
  For a multi-input operation, per-input alone is a hole — a hundred inputs each just under
  `max_input_bytes` is a hundred times the ceiling. `merge` checks input size per input *and*
  against the sum, and `max_pages` against the **output** total; ADR 0017 §3 is the worked
  example.
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
    //
    // The real signature, not `burrow_core::ops::<operation>(data, &Limits)` -- no operation
    // has ever had that shape, and this snippet claimed it until M1 PR B. An operation is
    // called on an ENGINE, with `OpenOptions` carrying the limits and the injected clock, so
    // a target has to construct one. `fuzz/fuzz_targets/merge.rs` is the multi-input
    // example, including how it splits one buffer into several inputs.
    let engine = /* the engine the operation runs on */;
    let _ = burrow_ops::<operation>(&engine, /* inputs derived from `data` */, &options);
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
- A Playwright test drives it with a real file, and the no-network assertion still passes —
  **against the tool page, not only against `/harness`**. The harness is deleted from
  production builds, so a console-silence or zero-requests assertion that only runs there says
  nothing about a route a person can visit.
- **Do not use an Astro `client:*` directive.** Hydration bootstraps the island with an
  *inline* script, and `script-src 'self'` carries no `'unsafe-inline'` and no nonce (ADR
  0014), so the browser refuses it and the tool renders as dead markup — with no error anyone
  would see except a CSP line in the console. Mount the component from a plain Astro
  `<script>` block, which Astro bundles to an external `_astro/*.js`.
- **Serve the build on the port the CSP was generated against** (4321 by default). `connect-src`
  names absolute engine URLs, so the same `dist/` on another port refuses every engine fetch —
  which looks exactly like a broken page. An hour went into this one in M1 PR B3.
- Say what happens to a file over the limit in the page's own prose, rather than letting
  someone discover it. And do not re-implement the ceiling in the island: send the files, and
  report what the core refuses, so the prose and the code can be caught disagreeing.

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
