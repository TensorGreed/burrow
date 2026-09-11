# 0011. Every PDFium call runs on one dedicated engine thread

Date: 2026-09-10

## Status

Accepted.

## Context

[ADR 0004](0004-native-engines.md) chose a prebuilt PDFium.
[M1 PR 1](https://github.com/TensorGreed/burrow/pull/14) linked it and proved it loads.
This decision is about how it is *called*, and it was forced by something PR 1 measured
rather than by a preference.

**PDFium's library init is global and not re-entrant.** Two tests each calling
`FPDF_InitLibrary` tripped an internal `CHECK` and the process died with `SIGTRAP` —
because `cargo test` runs tests on parallel threads. PR 1 fixed that specific call with a
`Once` and wrote down, in `link_check.rs`, that the constraint would carry into PR 2.

The `Once` is not enough, and the reason is easy to miss: **it only guards init.**
`FPDF_LoadMemDocument64`, `FPDF_GetPageCount` and `FPDF_CloseDocument` were all still
reachable from any thread at any time. PDFium is not documented as thread-safe for
concurrent use of the library at all — not merely at startup — so the crash PR 1 fixed was
the *loudest* instance of a general problem, not the whole of it. A quieter instance
would not crash; it would return a confidently wrong answer, which for M2's redaction is
the failure mode that matters.

There is a second, related global: **`FPDF_GetLastError()`**. It is process-wide state, so
any PDFium call between a failure and the read of its code overwrites the code. Serialising
calls is what makes "read the error immediately" a statement anyone can verify.

So every `FPDF_*` call has to be serialised. Two ways to do that:

**A process-wide mutex.** `static ENGINE: Mutex<()>`, taken around every call.

**A dedicated thread.** One OS thread owns the library; every call is a job submitted to
it and awaited.

## Decision

**A dedicated engine thread**, in `core/burrow-engines/src/pdfium/thread.rs`. It is
started once, on first use, by a `OnceLock`, and never stopped. Raw `FPDF_DOCUMENT`
handles live in a registry inside that thread and never leave it; callers hold a `u64`.

Four reasons, of which the second is the one that decided it:

1. **A mutex gives mutual exclusion. PDFium also has thread-local state.** Its allocator,
   among other things, is per-thread. One thread satisfies both properties; a mutex
   satisfies only one. The gap is undefined behaviour that would not reliably reproduce in
   a test, which is the worst kind to leave open.

2. **No `unsafe impl Send` anywhere.** `FPDF_DOCUMENT` is a raw pointer and is therefore
   not `Send`. Under a mutex, letting a caller open a document on one thread and use it on
   another — which `DocumentEngine::Document: Send + Sync` requires, and which callers
   will do — needs an `unsafe impl Send for PdfiumDocument`. That is an assertion a
   reviewer has to take on trust, about a property no test can check.

   With a thread, the pointer never crosses a thread boundary, so `PdfiumDocument` is an
   id and a deadline and derives `Send + Sync`. The registry entry holds the raw pointer
   and is consequently `!Send`, so **the compiler refuses** to let it escape the engine
   thread. The discipline is checked, not documented.

3. **`FPDF_InitLibrary` gets exactly one caller on exactly one thread**, structurally.
   `link_check.rs`'s stated process-wide obligation is discharged by construction rather
   than by everyone remembering it — and PR 1 predicted correctly that PR 2 would add a
   second caller.

4. **It is the same shape as the web implementation.** M1 PR 4 drives a separate
   Emscripten PDFium module across a JS bridge ([ADR 0006](0006-wasm-linking-strategy.md)):
   submit a request, await a reply, hold an opaque handle. Making the native side look the
   same keeps the two implementations of one trait readable side by side, which is what
   the differential conformance harness (ROADMAP M1 item 12) depends on.

### What follows from it

- **Document handles are `Send + Sync`, and are ids.** `PdfiumDocument` is `{ id: u64,
  deadline: Deadline, clock: Arc<dyn Clock>, pages_at_open: u64 }`. Dropping one submits a close without waiting,
  so a drop never blocks and can never deadlock.
- **The buffer is owned by the engine, not borrowed.** `open` takes `Box<[u8]>` by value
  and the registry entry holds it next to the handle, closing the document in `Drop::drop`
  before the buffer's field is dropped. PDFium requires the buffer to outlive the document
  (`fpdfview.h:451`); this makes that structural rather than documented. A borrowing
  `Document<'a>` would prove the same thing natively but forces a lifetime into the trait
  that the web implementation cannot honestly satisfy — it has nothing to borrow.
- **The clock is captured at open, not passed per call.** `page_count` deliberately takes
  no `&dyn Clock`. An earlier signature did, and it silently disabled `max_duration_ms`: a
  `Deadline` stores a start reading from one clock's *unspecified* epoch, so measuring it
  against a different clock made the elapsed time saturate to zero and the limit could
  never fire again — no error, no warning, and the repository's own test helper was already
  doing it. `OpenOptions` therefore takes an `Arc<dyn Clock>` and the document keeps it.
  This is the second time the handle-based shape paid for itself: the parameter that made
  the bug expressible simply does not exist any more.
- **A caught panic poisons the engine permanently.** Jobs run inside `catch_unwind`, so a
  panic becomes a typed `Error::Internal` for the caller rather than a stranded channel.
  But the engine's invariants may be broken, so it is never used again — the same rule
  [ADR 0009](0009-web-panic-contract-and-binding-boundary.md) states for a trapped web
  instance. The panic payload is dropped without being formatted: it can carry
  input-derived bytes.
- **A zero error code on a failed call maps to `Error::Internal`.** If
  `FPDF_LoadMemDocument64` returns null while `FPDF_GetLastError()` reads
  `FPDF_ERR_SUCCESS`, the engine has contradicted itself and we do not know what state it
  is in. `Internal` is fatal to a web worker under ADR 0009, which is the conservative
  response. Every failure PDFium *can* explain sets a non-zero code, so this is not a path
  real files take. The alternative — folding it into `Malformed` — never costs a worker,
  but buries a genuine engine anomaly in the most common bucket where nobody would ever
  see it.

  What must never happen is it reading as *success*. That is
  [ADR 0006](0006-wasm-linking-strategy.md) requirement 6 (`-FPDF_GetLastError()` yields
  `-0`, and `-0 === 0` in JS), and it has its own test.

## Consequences

Every engine call costs a channel round trip — microseconds, against milliseconds of
parsing. Not measurable against real work, and it is the price of the guarantee.

**Throughput is capped at one core for engine work, permanently.** A mutex would have the
same ceiling, so nothing is lost against the alternative, but it is a real limit: burrow
cannot parse two documents in parallel on native. If that ever becomes the bottleneck, the
answer is a *pool* of engine threads with documents pinned to one each — which this design
extends to and a mutex does not, because the handles are already ids rather than pointers.

Job closures must be `Send + 'static`, so anything an operation needs is moved into the
engine thread and moved back. For `open` that is the input buffer, which the engine keeps
anyway.

Poisoning is deliberately unrecoverable, and in a long-lived process one bug takes PDFium
out for good. That is the intended trade — ADR 0009 argues at length that reusing an
instance whose invariants broke is worse than a visible failure — but it means a panic in
the engine wrapper is a total outage rather than a degraded one, and it should be treated
with that severity.

### Two couplings this design creates, measured

Recorded because both were found by review rather than by design, and neither is visible
from the decision above.

**One slow document delays every other caller.** `submit` blocks on a reply and there is
one thread, so a hostile file that makes a single engine call run for seconds stalls
everything behind it. Measured: a 32 MB file with a bogus `startxref`, forcing PDFium's
cross-reference rebuild, made a concurrent open of a valid 200-byte file wait 716 ms — and
that caller was then told **its own** `max_duration_ms` of 1 ms had been exceeded, which is
a wrong answer rather than merely a slow one. The single thread is therefore an
availability coupling between unrelated documents, not only a throughput ceiling. The fix
is a bounded wait that distinguishes "the queue was busy" from "your own work was slow";
it is not in M1 PR 2.

**Nothing caps the registry.** `Limits` is per-operation, so N concurrently open documents
cost N times a single open, and the engine holds every input buffer until its handle is
dropped. A registry-level ceiling — on count, and on summed buffer bytes, checked in
`reserve_id` — is the answer. Also not in M1 PR 2.

**And an engine out-of-memory cannot be survived at all.** A thread shares its address
space with the process, so when PDFium hits its own allocation failure it `abort()`s and
takes everything with it. That is not a panic, so neither the `catch_unwind` above nor
`burrow-ffi::guard` sees it, and no amount of limit-checking in Rust changes it. Making an
engine OOM recoverable needs an engine **process** with `RLIMIT_AS`, which is a different
ADR — and is unavailable in a browser tab and unattractive on mobile, so the realistic
mitigation is to refuse the file before the allocation. That means a structural pre-scan of
the declared sizes, which needs a parser that is not PDFium: qpdf, in M1 PR 3.

`tests/parallelism.rs` is the regression test: 32 threads × 32 opens, a document opened on
one thread and read from several others, and many documents closing while others open. A
passing run is not proof that no data race exists, and must not be read as one. It is a
regression test for a failure that has actually happened here.

## Alternatives considered

**A process-wide mutex.** Discussed above. Rejected on the thread-affinity gap and on
needing `unsafe impl Send` for a property nothing can check.

**Initialise PDFium per operation.** Would remove the shared state entirely. Not possible:
`FPDF_InitLibrary`/`FPDF_DestroyLibrary` are process-global and the teardown is unsound
while anything could still be inside the library — which is why `FPDF_DestroyLibrary` is
never called at all.

**A thread pool with documents pinned to a thread.** More throughput, same safety, because
a given document would only ever be touched by its own thread. Rejected **for now**, not on
principle: PDFium's global init and `FPDF_GetLastError` would both need care across
threads, and there is no operation yet whose performance would justify the complexity. The
handle-based design leaves this open as a later change that touches only `thread.rs`.

**`#![forbid(unsafe_code)]` plus a safe PDFium wrapper crate** (`pdfium-render`). Would
move the problem to someone else. Rejected under
[ADR 0004](0004-native-engines.md)'s reasoning: a wrapper is a dependency whose licence,
supply chain, and thread-safety assumptions we would have to audit anyway, and the surface
we need is five functions.
