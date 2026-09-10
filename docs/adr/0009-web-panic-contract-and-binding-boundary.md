# 0009. The web panic contract, and where the binding boundary really is

Date: 2026-09-10

## Status

Accepted — amends [0002](0002-rust-core-and-bindings.md).

Written as a separate ADR rather than an edit, because [ADR 0001](0001-record-architecture-decisions.md)
makes accepted ADRs append-only. ADR 0002's architecture is unchanged and still correct;
two of its stated rules turn out to be wrong or incomplete on the web, and this records
what replaces them.

## Context

[ADR 0002](0002-rust-core-and-bindings.md) states two rules that
[spike 0001](../spikes/0001-wasm-engines.md) measured directly:

> No panic crosses an FFI boundary. `burrow-ffi::guard` wraps entry points in
> `catch_unwind` and maps panics to `Error::Internal`. The release profile keeps
> `panic = "unwind"` precisely so this is possible.

> Bindings contain no logic. They marshal types and enforce nothing, so there is only one
> implementation of every rule.

The first is **unachievable on the web** under the linking strategy
[ADR 0006](0006-wasm-linking-strategy.md) adopted. The second **survives, but not as
written** — it is not specific enough to survive contact with a page loop, and it was
already being read too loosely.

### What was measured

On `wasm32-unknown-unknown` a Rust panic ends in the `unreachable` instruction. That is a
WebAssembly **trap**, not an unwind:

```
PANIC_REPLY {"ok":false,"threw":"RuntimeError: unreachable"}
DEATHS 0
AFTER_PANIC       {"pages":1,"isOk":true}
AFTER_PANIC_QPDF  {"pages":100,"isOk":true}
```

Three consequences, none of which ADR 0002 anticipated:

1. **`catch_unwind` cannot catch it.** There is no unwinding. `panic = "unwind"` in the
   release profile buys nothing on this target, so `guard` — which is correct and
   necessary for Swift and Kotlin — is inert on the web.
2. **The worker does not die.** A trap surfaces in JS as a catchable `RuntimeError`, no
   `error` event fires, and the worker keeps serving requests. ADR 0002's implied model of
   "panic → crashed host" is wrong here.
3. **Calls afterwards appear to succeed.** They did in the spike — but only because its
   Rust held no state, and because under option 1 the engines live in *separate* modules
   whose heaps the trap never touched. The Rust instance's invariants are broken
   regardless.

There is a second, sneakier failure with the same required response: an Emscripten
`abort()` **inside an engine module** throws a JS exception, which becomes an ordinary
`Error::Internal` reply. The worker stays alive with its init flags still set, so nothing
re-initialises — and one crafted PDF silently bricks that engine for the rest of the
session while the page keeps feeding it work.

Notably, `catch_unwind` **does** work on `wasm32-unknown-emscripten`, which is one of the
reasons ADR 0006 records option 2 as the better architecture and schedules a pre-M2 gate.

## Decision

### 1. The web panic contract

On the web, the guarantee is **not** "no panic crosses the boundary". It is:

> **Any `Error::Internal`, or any trap observed as a JS exception from the wasm module,
> is fatal to that instance. The page terminates the worker and spawns a fresh one before
> the next operation. An aborted or trapped instance is never reused.**

This is a *page-level* contract enforced by the worker lifecycle, not a core-level one
enforced by `catch_unwind`. It has to be, because on this target Rust cannot observe its
own panic.

Consequences that follow, and must be implemented rather than assumed:

- **Recovery is the page's decision, and nothing forces its hand.** Because the worker
  survives, there is no event to react to. The page must treat the *result* as the signal.
- **`Error::Internal` must be distinguishable from every other error.** `Malformed`,
  `LimitExceeded` and `PasswordRequired` are normal outcomes and must not cost a worker.
  `Internal` always does.
- **In-flight requests on a discarded worker fail with `Internal`**, and are not retried
  automatically. A retry loop against a poisoned instance is worse than a visible failure.
- **Never surface the trap or abort text.** It is panic output and can carry
  input-derived bytes — the same reason `burrow-ffi::guard` was changed to discard the
  panic payload. Report the same content-free `Internal`.

`burrow-ffi::guard` is unchanged and still required: it is correct and load-bearing for
iOS and Android, where unwinding works.

### 2. Where the binding boundary really is

ADR 0002's "bindings contain no logic" stands, and the spike is evidence it is keepable —
the JS bridge is about 60 lines and genuinely mechanical: allocate in the engine heap,
copy, call one function, free. Every decision stayed in Rust, including the whole error
taxonomy.

But "no logic" is too vague to survive the first multi-step operation, so it is replaced
with something testable:

> **Bindings may hold engine handles and marshal data across the boundary. No branch on
> engine state may live in JS.**

Concretely, a binding **may**: allocate and free in an engine heap; copy bytes in and out;
hold an opaque engine handle across calls; translate a value's representation; and pass
through an engine's return value unexamined.

A binding **may not**: interpret an engine error code; decide which engine to call; decide
whether to retry, repair, or fall back; loop over pages deciding when to stop; or enforce
any part of `Limits`.

The practical effect on multi-step work — open, iterate pages, write output — is that the
handle stays alive across many JS round trips with **Rust orchestrating each one**. That
costs call overhead. Moving the loop into JS would be faster and is forbidden, because it
puts a decision in the one place we cannot test with `cargo test` and cannot reuse on iOS
or Android.

## Consequences

The web's failure model is now written down and differs from mobile's. That asymmetry is
real and permanent under option 1: on iOS and Android a panic becomes a typed error and
the app continues; on the web it costs a worker. `apps/web/CLAUDE.md` carries the concrete
requirements.

The cost is that the web's safety property is enforced by page code rather than by the
type system. Nothing in Rust can assert it. It needs a test — one that panics deliberately,
asserts the page discards the instance, and asserts the next operation succeeds on a fresh
one. That test is a requirement of ADR 0006, not an optional extra.

The restated binding rule is stricter in effect than "no logic" was in practice, because it
names the specific temptation (a page loop in JS) and forbids it. Expect it to be
inconvenient exactly once per operation, and expect pressure to relax it for performance.

If the pre-M2 gate adopts option 2, most of section 1 becomes unnecessary: `catch_unwind`
works there, and ADR 0002's original rule holds on the web as written. Section 2 stands
either way.

## Alternatives considered

**Set `panic = "abort"` on the web and treat every panic as fatal to the worker.** Honest
about the target, and simpler. Rejected: it makes the worker die where today it survives,
which loses the ability to report a typed `Internal` to the user at all — and the page
still has to handle the engine-`abort()` case, which is not a Rust panic.

**Wrap every wasm-bindgen entry point in a JS `try`/`catch` and map the trap to
`Error::Internal` there.** Tempting, and it looks like it restores ADR 0002's rule.
Rejected: it is error classification in JS, which section 2 forbids, and it would report a
poisoned instance as a recoverable error — the worst outcome, because the caller would
keep using it.

**Rely on the trap being recoverable, since calls after a panic succeeded in the spike.**
Rejected outright. They succeeded because the spike's Rust was stateless and the engines
were in other modules. Treating an observation from a stateless probe as a guarantee about
a stateful implementation is how a redaction bug ships.
