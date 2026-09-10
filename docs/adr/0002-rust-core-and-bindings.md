# 0002. A shared Rust core with uniffi and wasm bindings

Date: 2026-09-10

## Status

Accepted

## Context

burrow ships the same file operations to three places: a website, an iOS app, and an
Android app. Those operations are parsers and rewriters of hostile binary formats. Two
properties dominate every other consideration:

1. **The privacy guarantee must be structural.** File content never leaves the device. If
   the same operation is implemented three times, that guarantee has to be re-proved
   three times, and the third implementation is where it breaks.
2. **The code is a memory-safety liability.** PDF, JPEG, and video parsers are the
   classic source of exploitable bugs. Correctness matters most in redaction, where a
   subtle bug leaks the very secret the user was removing.

Three implementations also means three sets of golden files, three fuzzing budgets, and
three places for a redaction bug to hide. Meanwhile the frontends genuinely differ: file
pickers, share sheets, progress reporting, and platform OCR are not portable, and users
notice when an app is not native.

## Decision

We will implement every operation once, in a Rust workspace under `core/`, and expose it
to each platform through a thin binding layer:

- **`bindings/burrow-ffi`** — [uniffi](https://mozilla.github.io/uniffi-rs/) generates
  Swift and Kotlin bindings from the Rust API. Chosen over hand-written FFI or cbindgen
  because it generates both languages from one definition and handles the error mapping
  we need.
- **`bindings/burrow-wasm`** — [wasm-bindgen](https://wasm-bindgen.github.io/wasm-bindgen/)
  for the web, running in the browser tab.

The core is split so that the boundary is auditable in one place:

| Crate | Role |
|---|---|
| `burrow-types` | Typed errors and `Limits`. No I/O, no engines, no dependencies of substance. |
| `burrow-engines` | The only crate that wraps C/C++ engines, and the only core crate allowed `unsafe`. |
| `burrow-ops` | Operations, written against the engine traits rather than against PDFium's C API. |
| `burrow-core` | The public surface. **The only crate the bindings depend on.** |

Rules that follow from this, and which are enforced rather than assumed:

- Crates are `#![forbid(unsafe_code)]` by default. Only `burrow-engines` and
  `burrow-ffi` relax it, to `#![deny(unsafe_op_in_unsafe_fn)]`, and every `unsafe` block
  carries a `// SAFETY:` comment.
- No panic crosses an FFI boundary. `burrow-ffi::guard` wraps entry points in
  `catch_unwind` and maps panics to `Error::Internal`. The release profile keeps
  `panic = "unwind"` precisely so this is possible; `panic = "abort"` would turn a
  recoverable bug into a crashed host app.
- Bindings contain no logic. They marshal types and enforce nothing, so there is only one
  implementation of every rule.

**The frontends are native.** No WebView wrappers: Android is Jetpack Compose, iOS is
SwiftUI, the web is a real website. Platform capabilities that are better on-device than
anything we would write — PDFKit, Vision, AVFoundation, ML Kit, Media3 — are used *from
the app layer*, never from the core, so the core stays portable and testable headlessly.

## Consequences

One implementation to fuzz, one corpus of golden files, one place a redaction bug can
live, and one place to audit for network calls. Testing the core headlessly on Linux
covers the logic for all three platforms, which is what makes the self-hosted ARM64
regression machine worth having.

The costs are real. The FFI boundary is the highest-risk code in the project and needs
the most careful review. Rich types cross that boundary awkwardly, so the public API must
stay deliberately narrow and value-oriented — this constrains API design permanently, not
just at first. Contributors need Rust plus their platform language. Build complexity is
significant: cross-compiling to `aarch64-apple-ios`, two Android ABIs, and
`wasm32-unknown-unknown`, with vendored C/C++ engines for each. iOS builds require macOS,
so they cannot be produced on the Linux dev machine at all.

Using platform OCR from the app layer means text recognition results will differ between
iOS, Android, and web. That is an accepted, visible inconsistency; the alternative is
bundling a recognition engine into the core and being worse than the platform on two of
three targets.

## Alternatives considered

**Native implementation per platform.** Best-integrated, and the fastest route to a good
first app. Rejected: three parsers for hostile formats is three times the attack surface
and three chances to leak a redaction, which is the one thing we cannot get wrong.

**One web app wrapped in a WebView on mobile.** Cheapest path to three products. Rejected
because it is a worse product — file pickers, share sheets, and background handling are
all degraded — and because it puts the operations in a runtime we control least.

**C or C++ core.** Would reuse the engines' own language and skip a marshalling layer.
Rejected: the memory-safety profile is the problem we are trying to avoid, and we would
be hand-writing bindings for three platforms anyway.

**Go or Kotlin Multiplatform core.** Both viable for portability. Rejected on wasm output
size and on the difficulty of tightly controlling allocation and linking against the
C/C++ engines we depend on.

**cbindgen with hand-written Swift/Kotlin wrappers instead of uniffi.** More control over
the generated API. Rejected: two hand-maintained binding layers is exactly the
duplication this ADR exists to avoid.
