# 0007. Per-platform enforcement of time and memory limits

Date: 2026-09-10

## Status

Accepted. The native half landed in M1 PR 2: `burrow_types::Clock`, `ManualClock` and
`Deadline` are the injectable clock; `burrow-engines`' `pdfium::estimate` is the
estimate-based memory pre-check; and `Limits`' rustdoc says what each field actually
guarantees.

Two parts of this ADR are **not** implemented yet, and are not claimed to be:

- The **web** mechanisms — `performance.now()` as the clock, and `max_memory_bytes` as the
  WASM instance's maximum memory, which is the one place it is a hard ceiling. Those land
  with the web path in M1 PR 4.
- Wiring the deadline into an **engine progress or abort callback** to tighten the
  granularity. PDFium exposes one; nothing uses it yet, so enforcement is exactly as
  coarse as this ADR says — one engine call.

## Context

`Limits` currently promises five ceilings: `max_input_bytes`, `max_memory_bytes`,
`max_duration_ms`, `max_pages`, and `max_pixels`. Three of those are cheap and honest —
input size, page count, and pixel count are all numbers we read and compare before
acting.

The other two are not, and the rustdoc as originally written overstated what we can
deliver.

**`max_duration_ms` has no portable clock.** The obvious implementation is
`std::time::Instant::now()`. On `wasm32-unknown-unknown` there is no clock: `Instant::now()`
**panics**. Since the core forbids panics and the web is our first target, a limit
implemented with `Instant` would be a guaranteed crash in the browser — and, because
`wasm32-unknown-unknown` aborts rather than unwinds (see
[ADR 0006](0006-wasm-linking-strategy.md) and `apps/web/CLAUDE.md`), it would take the
whole worker with it.

Nor is a deadline check enough on its own. Time is only observed where we look at it, so
a single long call into PDFium cannot be interrupted by a Rust-side check. Enforcement
granularity is bounded by how often control returns to us.

**`max_memory_bytes` is not observable from Rust.** The allocations that matter are made
by C++ inside PDFium and qpdf, through their own allocators. Rust's allocator never sees
them, so no `GlobalAlloc` wrapper, counter, or RAII guard in `burrow-engines` can
measure — let alone cap — the memory an engine consumes. A tracking allocator would
report a small number while the process was in fact using gigabytes, which is worse than
reporting nothing.

The consequence of getting this wrong differs per platform, which is what makes it an
architectural question rather than an implementation detail:

| Platform | Consequence of unbounded use |
|---|---|
| Web | The worker is killed, or the tab OOMs |
| iOS | The OS terminates the app, with no chance to report |
| Android | Same, plus a much lower per-app heap ceiling |
| Linux (corpus runs) | The OOM killer takes the process mid-run |

## Decision

We will enforce both limits with **platform-specific mechanisms behind one portable
API**, and we will document precisely what each guarantees rather than implying a
uniform one.

### `max_duration_ms`: an injectable clock

Time is a dependency, not an ambient fact. `burrow-types` defines a minimal clock trait;
operations take one and consult it at their checkpoints.

- **Native** (iOS, Android, Linux): backed by `Instant`.
- **Web**: backed by `performance.now()`, supplied through the binding layer. Never
  `Instant`, which panics.
- **Tests**: a deterministic fake clock, which is the other reason for this shape — a
  timeout test that really waits 60 seconds is a test nobody runs.

Enforcement is **cooperative and checkpoint-based**. We will document it that way: the
limit is checked at page boundaries and between engine calls, so the effective overshoot
is one engine call. A hostile file that makes a *single* PDFium call run for a minute is
not stopped by this, and we will not pretend otherwise. Where an engine offers a progress
or abort callback, we will wire the deadline into it to tighten the granularity.

### `max_memory_bytes`: a ceiling on the web, an estimate elsewhere

- **Web**: the real mechanism. The WASM instance is created with a `maximum` memory size
  derived from `max_memory_bytes`, so growth past it fails inside the sandbox rather than
  taking the tab down. This is a genuine hard ceiling, and it is why the web is the
  best-protected platform here — the inverse of the usual expectation.
- **Native**: **estimate-based pre-checks**. Before an operation, compute an expected
  cost from observable inputs — page count, page dimensions, decoded pixel counts,
  embedded image sizes, compression ratios — and reject up front when the estimate
  exceeds the limit. Combined with the existing `max_input_bytes` and `max_pixels`
  checks, this catches the amplification attacks that matter (a small file declaring an
  enormous raster) without claiming to meter C++ allocation.
- Where an engine exposes an allocator hook, we will use it. PDFium's is worth
  investigating during the M1 spike.

### Documentation must match

`Limits`' rustdoc is updated in this change so it no longer promises enforcement we
cannot deliver. Each field states its mechanism and its granularity. A caller who reads
`max_memory_bytes` and assumes a hard cap on native would build on a false guarantee —
and a limit believed to be stronger than it is, is worse than a documented weak one.

## Consequences

Operations gain a clock parameter. That is a small ergonomic cost repaid immediately in
testability: timeout behaviour becomes a fast unit test rather than a slow, flaky one.

The guarantees are genuinely uneven, and now visibly so. On the web,
`max_memory_bytes` is a hard ceiling and exceeding it is a clean, recoverable failure. On
native it is a best-effort pre-check, and a sufficiently adversarial file can still get
the app killed by the OS. Callers on constrained devices should set conservative limits
rather than trusting the ceiling — which the rustdoc now says.

Estimation is a real, ongoing cost: every operation needs a cost model, the models will
be wrong at first, and refining them is what the corpus regression runs are for. An
estimate that is too generous fails to protect; one that is too strict rejects legitimate
files, which users experience as a bug.

Because `max_duration_ms` is checkpoint-based, the fuzz targets cannot rely on it to
catch hangs. They need libFuzzer's own `-timeout`, and a hang found that way is a bug in
the operation's checkpoint placement, not just in the input.

## Alternatives considered

**A tracking global allocator.** The standard Rust answer, and it works well in
all-Rust programs. Rejected because the allocations we care about are C++ ones it cannot
see; it would produce confident, wrong numbers.

**A watchdog thread that kills a long operation.** Real preemption, not cooperative.
Rejected: no threads on `wasm32-unknown-unknown` in our configuration, killing a thread
mid-C++-call leaves engine state unsound, and the failure is not recoverable into a typed
error.

**Run each operation in a separate OS process with `rlimit`.** A genuine hard cap, and
the right answer for a server. Rejected: there are no processes in a browser tab, and
spawning one per operation on iOS or Android is not available to us either.

**Drop `max_memory_bytes` from `Limits` since we cannot enforce it uniformly.** Honest,
and tempting. Rejected because the web enforcement is real and valuable, and because the
native estimate — while weaker — does stop the amplification cases that cause most
crashes. Documenting the difference is better than discarding the protection.
