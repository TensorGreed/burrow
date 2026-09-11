# 0015. The web worker lifecycle: watchdog, recovery, circuit breaker and recycling

Date: 2026-09-11

## Status

Accepted — amends [ADR 0007](0007-limit-enforcement-per-platform.md)'s `max_duration_ms`
section and completes [ADR 0009](0009-web-panic-contract-and-binding-boundary.md)'s web
contract.

Written as a separate record rather than an edit, because
[ADR 0001](0001-record-architecture-decisions.md) makes accepted ADRs append-only. ADR 0007's
decision is unchanged; what changes is that its web half now exists, and that one of its
statements about *whose* time is being measured needed making precise.

## Context

[M1 PR 4a-i](https://github.com/TensorGreed/burrow/pull/20) landed the web path's loading
half — three Emscripten modules compiled under a generated CSP, a `blob:` worker that fails
closed if no policy is in force, and a page count coming back from three real browsers. It
deferred five things, and what was missing was not features but **guarantees the existing ADRs
already claimed**:

- **ADR 0007** says `max_duration_ms` is checkpoint-based, and is explicit that it cannot
  interrupt one long engine call: *"A hostile file that makes a single PDFium call run for a
  minute is not stopped by this, and we will not pretend otherwise."* Nothing bounded such a
  call. A file that hung PDFium hung the tab, with no recovery path at all.
- **ADR 0009** says an `Internal` result is fatal to the instance and costs a worker, and that
  the page must enforce it because nothing else can — on `wasm32-unknown-unknown` a panic is
  an uncatchable trap, the worker survives it, and no event fires. That contract existed as
  forty lines of ad-hoc page code inside a test harness, with no state model and no tests.
- **ADR 0014 §3** states the guarantee as *"no requests at all after engine init — enforced by
  test"*, and names that test as this PR's. Until it existed, `e2e/csp.spec.ts`'s deliberate
  query-string exfiltration was an open hole with nothing closing it.
- **ROADMAP item 7** records engine logging suppressed at both layers, with the thorough
  console-silence test assigned here.

There was also a problem neither ADR had a name for. WASM linear memory **only grows** — an
Emscripten module never returns a page to the host. `check_measured_memory` handles the cost of
*one* operation; nothing handled accumulation. A hundred files that each cost 40 MB never trip
a per-operation limit, and the worker's heap is 4 GB.

## Decision

### 1. The main thread holds an explicit state machine

`apps/web/src/host/worker-host.js`. Five states:

| State | Meaning |
|---|---|
| `idle` | a live, initialised worker, doing nothing |
| `initialising` | the **first** worker is spawning and running engine init |
| `busy` | a live worker is running one operation |
| `dead` | no worker; the next request must respawn, or be refused |
| `respawning` | a **replacement** worker is spawning and running engine init |

`initialising` and `respawning` do identical work and are deliberately distinct: "a request
arrived while a respawn was in progress" is one of the cases that must be observable, and
collapsing them would make it unobservable.

**Every dependency is injected** — `spawn`, `now`, `setTimer`, `clearTimer`. No `Worker`, no
`performance`, no DOM. That is [ADR 0007](0007-limit-enforcement-per-platform.md)'s reason for
an injectable clock applied to the page: *"a timeout test that really waits 60 seconds is a
test nobody runs"*. `worker-host.test.ts` drives all 25 cases in 46 ms.

The fake worker **accounts for what it hands out** — instances, terminations, unsettled
requests — and `assertNoLeaks()` runs after every case. This is a direct lesson from 4a-i,
whose Rust fake returned a fixed logger handle and was structurally incapable of noticing the
per-operation logger leak it was meant to cover.

### 2. The watchdog's clock starts when the worker takes the operation

The worker posts `{ id, ack: true }` once `ensureReady()` has resolved and immediately before
it begins. The page's timer starts **on that ack**, not on its own `postMessage`.

Everything before the ack — the fail-closed policy guard, and a cold 6.5 MB compile across
three modules — is **start-up**, bounded separately by `initTimeoutMs` (60 s). A file is never
blamed for time spent before the worker could look at it. This is the web form of the queue-time
bug PR 2 fixed natively, where a caller delayed behind someone else's work was told its own
deadline had expired.

The ack goes out **before** `blob.arrayBuffer()`, because reading the file *is* the operation:
a 100 MB blob takes real time and that time is attributable to the file, unlike the compile.

- On `max_duration_ms` + 500 ms grace: terminate, and fail the caller with `LimitExceeded`
  naming `max_duration_ms` with both numbers — the same shape Rust produces at a checkpoint,
  because the user hit the same wall and should not meet two vocabularies depending on which
  side noticed.
- On `initTimeoutMs`: terminate, and fail with `Internal`. **Never a duration limit** —
  start-up failing is not the file's fault, and reporting it as one would tell a user to shrink
  a document nothing ever looked at.
- The budget is **per operation**, not per host, because `Limits` are: a page may run a 2 MB
  file and a 200 MB one under different ceilings.

**Nothing is ever retried.** ADR 0009 already forbids it after `Internal`; the watchdog joins
that rule for the same reason — a file that hangs one worker hangs the next.

#### 2a. Operations are serialised, because the worker is

Found by security review, and it is the same bug the ack exists to prevent, one window earlier.

A request is also bounded *before* its ack — everything between `postMessage` and the ack is
the worker getting to the request, so it is governed by `initTimeoutMs` and its expiry counts
as a crash. With concurrent operations that was wrong in a way that compounds: the worker's
message loop is single-threaded, so a second operation could not be acked until the first
finished, and **the second's pre-ack timer fired on the first** — terminating a healthy worker
mid-operation, reporting `Internal` to a caller whose own budget had minutes left, and counting
a crash. Three of those latch the breaker and the page refuses to work at all. Measured against
the real host: two concurrent operations with 300 s budgets, dead at 60 s.

So `run()` queues. Only one operation is posted at a time, which is what the worker does
anyway; a request waiting in the queue carries **no timer at all**, because nothing about
waiting is the file's fault, and `discardWorker()` is how a page cancels.

A consequence worth naming: a queued request is **not** collateral damage when the running one
poisons its worker. It never touched that worker, so it waits and runs on the replacement.
ADR 0009's "fail every in-flight request on a discarded worker" still holds — there is simply
never more than one in flight.

#### 2b. An ack is once, and never for an init

Two hardening fixes from the same review. Neither is reachable from file content today —
`main.js` acks exactly once per operation, never for an init, and its source is
integrity-pinned — and both are refused by the host so that the guarantee belongs to the host
rather than to the bundle:

- **A repeated ack is ignored.** A worker that re-acked would push its deadline out
  indefinitely and the operation budget would cease to exist: a hang with no recovery, which is
  exactly what the watchdog is here to close.
- **An ack on the init request is ignored.** Otherwise it would swap the start-up bound for the
  operation budget and then report a slow start as `LimitExceeded / max_duration_ms` — the
  inversion this section forbids in as many words.

### 3. The circuit breaker counts crashes, not respawns

After **3 crashes within a 60 s sliding window** the breaker opens, every later request returns
`EngineUnavailable`, and nothing is spawned until `host.reset()`.

**It latches.** Time alone does not reopen it. "Stop respawning" has to mean stopped: a page
retrying on a timer would resume the crash loop at a slower rate rather than end it. A UI wires
`reset()` to a deliberate gesture — a "try again" button — so another worker is only ever spent
because a person decided to spend it.

**Crashes, not respawns, and that distinction was found by measurement rather than review.**
Counting respawns was the first implementation. A recycle is a respawn, and so is a
page-initiated cancel — so `e2e/measure.spec.ts` recorded two of its five respawn samples as
`0 ms`, because three recycles had already opened the breaker. A page working through a run of
large files would have taken itself offline while every worker it had was perfectly healthy.
Only an involuntary death — a fatal reply, a watchdog kill, a worker `error` event, or a failed
start-up — is evidence of a loop.

`EngineUnavailable` is a **host lifecycle verdict, not an `Error` variant.** Every other `kind`
a reply carries is a classification of something an engine returned, computed in Rust by
`burrow-wasm`'s `kind_of`, because ADR 0009 forbids a binding classifying an engine error. This
one classifies nothing about any file: it describes the page's own willingness to hand out
another worker, which is a fact only the page has.

### 4. The input is a `Blob`, never a transferred `ArrayBuffer`

Structured clone passes a `Blob` **by reference**. Two consequences, and the second is why it
is a lifecycle decision rather than a performance one:

- The main thread never materialises the file bytes at all.
- **The caller still holds a usable handle after its worker is killed.** A transferred
  `ArrayBuffer` is detached page-side and gone — so "retry on a fresh worker" would be
  impossible for exactly the files that needed it.

The worker reads it with `await blob.arrayBuffer()` inside its try block. **This is not a
network request**: reading a Blob is a memory read, no CSP directive is consulted, and no
request is issued. Asserted rather than assumed — `e2e/zero-requests.spec.ts` delivers the whole
corpus this way and watches the server's own accept log.

### 5. Heap-growth recycling, with the threshold measured

A worker whose engine heap has grown past a ceiling is terminated **after** its result is
delivered, and the next request spawns a fresh one. Recycling is not a failure and the caller
must never see it as one.

The verdict is computed **in Rust**, on `Reply`, beside `is_fatal` — `core/burrow-engines/src/web/recycle.rs`.
The threshold derives from `Limits::max_memory_bytes`, and ADR 0009 is explicit that a binding
may not enforce any part of `Limits`. The worker forwards `recycle`; the page acts on it;
nothing in JavaScript compares a heap against anything.

Threshold: **the smaller of half `max_memory_bytes` and 512 MiB**, per engine heap.

- *Half*, because there are **two** independent engine heaps in a worker. A per-heap ceiling of
  the whole limit would let the pair reach twice it.
- *512 MiB*, because `Limits::default()`'s `max_memory_bytes` is generous enough on a desktop
  that half of it is a heap no phone survives, and a caller may set it higher still. Without an
  absolute floor, a permissive limit would silently mean "recycling off".

**There is a lower bound too, and it is not clamped.** A worker that has done any real work
sits at ~18 MiB per engine (§6), so a `max_memory_bytes` under about 36 MiB puts the threshold
below the baseline: the first operation on a fresh worker exceeds it, every operation costs a
respawn, and the heap never gets under the line. `MIN_CONVERGING_MEMORY_BYTES` (64 MiB) records
the number and a test pins the behaviour. It is deliberately **not** clamped — silently ignoring
a ceiling the caller set would be worse than honouring one they will notice — and it is the
number any mobile default proposed under §7 has to clear.

### 6. What the measurements say

Recorded here because §5's numbers and §2's "start-up is not the file's fault" both rest on
them. Chromium/Firefox/WebKit, `e2e/measure.spec.ts`, this machine (linux-aarch64):

**Respawn cost** — discard to `engines ready`, including re-fetching 6.5 MB from a `no-store`
server and recompiling all three modules, median of five:

| Browser | Median |
|---|---|
| Chromium | 73 ms |
| Firefox | 102 ms |
| WebKit | 81 ms |

**So the compiled-module optimisation is not taken.** Passing pre-compiled
`WebAssembly.Module`s to new workers would require re-deriving ADR 0014 §1a — the manifest is
generated *into* the bundle precisely because `pdfium.js` begins instantiating as it is parsed,
so a `postMessage` cannot arrive in time, and moving the integrity-pinned fetch to the page
would have to be shown to keep both SRI and the in-worker policy guard intact. That is a real
cost to pay for ~100 ms on a path taken only after a crash, a watchdog kill, or a recycle. The
measurement is recorded so the question does not have to be reopened from scratch; the bound in
`measure.spec.ts` fails if it ever stops being true.

**Engine heap growth**, every conformance fixture through both operations, `max_memory_bytes`
raised to 4 GiB so nothing is refused before an engine can grow:

| Input | PDFium | qpdf |
|---|---|---|
| every ordinary fixture, both operations | 17.9 MiB | 16 MiB |
| `xref-bomb.pdf` via `page_count` | **1900.7 MiB** | 16 MiB |
| `xref-bomb.pdf` via `structure_check` | 17.9 MiB | **513 MiB** |

Two orders of magnitude apart with nothing in between, identical in all three browsers. That
gap is what makes 512 MiB a threshold rather than a guess: any value between ~64 MiB and
~512 MiB separates the populations the same way.

### 7. The mobile-browser finding, which is not decided here

**On iOS a memory spike kills the whole tab, not just the worker**, and the user loses the page
and whatever they were doing with it. The table above is the reason that matters: a 330 KB file
drives PDFium to **1.9 GiB** in a browser worker whenever a caller has raised
`max_memory_bytes` above what the pre-scan would otherwise refuse.

Under `Limits::default()` the pre-scan refuses `xref-bomb.pdf` before PDFium allocates, and
`e2e/engines.spec.ts` pins that. So the exposure is not the default path — it is any page that
sets a more generous ceiling, and any bomb whose declared cost the pre-scan does not model.

**Desktop WebKit is not iOS**, and no number above was measured on a phone. Proposing mobile
defaults from these readings would be exactly the guess this ADR was asked not to make, so
per-platform `Limits` defaults for mobile browsers are **deferred to a decision with the
numbers in front of it**, not taken here. What this ADR does establish is that the recycling
ceiling is in place, that it is reachable by a real file, and that ordinary work sits two orders
of magnitude below it.

### 8. An accepted cost: the browser logs the guard's own probe

The fail-closed guard (ADR 0014 §1b) establishes that a policy is in force by making a request
the policy must refuse. **Browsers log refusals**, so on every worker spawn:

- **Firefox:** `Content-Security-Policy: The page's settings blocked the loading of a resource
  (connect-src) at …/__csp-probe …`
- **WebKit:** `Refused to connect to …/__csp-probe because it does not appear in the
  connect-src directive of the Content Security Policy.` (unhyphenated — the first version of
  the console-silence filter matched only Firefox's spelling and WebKit failed)
- **Chromium:** nothing surfaced from a worker.

ADR 0014 §5 rejected `frame-ancestors` in the meta tag because a per-page-load CSP console error
"would be noise that teaches a reader to ignore CSP console errors". This is the same noise
from a different direction, and the trade comes out the other way: the guard is what stops an
unpoliced worker from touching a file. One line per spawn is worth that.

`e2e/console-silence.spec.ts` excludes these **by probe path, not by prose**, and searches the
excluded lines for the canary separately — an exclusion that could swallow file content would
be worse than no assertion.

### 9. Engine log suppression is genuine defence in depth, measured

A mutation sweep removed each layer on its own and the console-silence test stayed green:

- `printErr`/`print` un-stubbed on both modules, C++ suppression intact — silent.
- `qpdf_silence_errors`, `qpdf_set_suppress_warnings` and the discarding logger all removed,
  `printErr` still stubbed — silent.
- **Both removed** — twelve lines, including `WARNING: input (offset 9): xref not found` and
  `WARNING: input, object 3 0 at offset 131: kid 0 (from 0) Resources is missing or invalid;
  repairing`.

So ADR 0006 requirement 2's "at both layers" is two independent mechanisms, not one with a
spare. The consequence worth stating: the test cannot say *which* layer regressed, only that one
of them is the last one standing.

### 10. How 4b records an expected difference

**Designed here, implemented in 4b.** The native path cannot interrupt a hang — ADR 0007's
enforcement is checkpoint-based and overshoot is one engine call — while the web path's
watchdog can. So a hang fixture *must* diverge: native hangs, web returns `LimitExceeded`.

A skipped test would record that as "untested", which is the wrong memory to leave for M2, where
a divergence between the two `DocumentEngine` implementations is a redaction bug rather than a
test failure.

`tests/conformance/expectations.json` gains an optional per-case block naming the platform, its
expected typed outcome, and a **mandatory reason**:

```json
{
  "name": "hangs-in-pdfium",
  "file": "fixtures/hang.pdf",
  "expect": { "err": "Timeout" },
  "platform_expectations": [
    {
      "platform": "web",
      "expect": { "err": "LimitExceeded", "limit": "max_duration_ms" },
      "reason": "the main-thread watchdog terminates the worker; the native path has no way to interrupt a single engine call (ADR 0007, ADR 0015 §2)"
    }
  ]
}
```

The differential harness then asserts the **recorded** difference rather than equality, and
fails both when a case diverges *without* a recorded reason and when a recorded difference stops
happening. The schema version bumps when 4b lands the first such fixture, and both readers —
`apps/web/e2e/engines.spec.ts` and `core/burrow-engines/tests/conformance.rs` — change together.
No fixture and no schema change in this PR.

### 11. `fatal` is false for `EngineUnavailable`

`fatal` means "this result poisons the engine instance". For a refusal there is no instance and
none will be made, so `true` was simply wrong — and it was not harmless: it made a browser test
that expected a crash pass on a refusal, because the two looked identical. Every other
host-produced failure keeps `fatal: true`.

## Consequences

The web now has a stronger duration guarantee than native, which inverts the usual
expectation and matches ADR 0007's note that the web is the best-protected platform for memory
for the same structural reason: a worker can be killed, a thread mid-C++-call cannot. ADR 0007's
"no threads on `wasm32-unknown-unknown`, and killing a thread mid-C++-call leaves engine state
unsound" rejected a watchdog *inside* the sandbox; a watchdog *outside* it has neither problem,
because the whole sandbox goes.

`max_duration_ms` therefore means two different things by platform, and this is the second such
asymmetry after `max_memory_bytes`. Both are now documented rather than implied.

The page holds real state, and it is the only place ADR 0009's contract lives. Nothing in Rust
can assert it; `worker-host.test.ts` and `e2e/recovery.spec.ts` are what hold it, and a change
to either needs the other checked.

A latching breaker means a page can reach a state where it refuses to work until told
otherwise. That is deliberate and it is a UI obligation: a tool page must surface
`EngineUnavailable` as something a person can act on, not as a generic failure.

## Alternatives considered

**Start the watchdog when the request is posted.** One fewer message and one fewer state.
Rejected: it charges the file for queue time and for a cold engine compile, which is precisely
the bug PR 2 fixed on the native path. A user would be told to shrink a document whose real
problem was a slow first load.

**Retry once after a watchdog kill.** Friendlier when the cause was a transient stall.
Rejected: a hostile file hangs the second worker exactly as it hung the first, and the page has
no way to tell the two causes apart. ADR 0009 already made this call for `Internal`; applying a
different rule to the watchdog would mean two policies for one situation.

**Let the breaker close when its window expires.** Rejected in §3 — it converts "stop" into
"slow down", which is not what the protection is for.

**Compute the recycle verdict in JavaScript from a reported heap size.** Simpler, no Rust
change, no wasm API surface. Rejected: the threshold derives from `Limits::max_memory_bytes`,
ADR 0009 forbids a binding enforcing any part of `Limits`, and a number living only in
JavaScript is testable only by vitest and free to drift from the native path's memory reasoning.

**Recycle on a fixed absolute ceiling only, ignoring `Limits`.** One number, no arithmetic.
Rejected: a page that deliberately sets a small `max_memory_bytes` — a mobile page, given §7 —
is asking for tighter behaviour and would not get it.
