# Exposure assessment: qpdf memory-unsafety on the open path (#62)

Date: 2026-09-14. Written after the private disclosure went to upstream
(`GHSA-m6gh-7c39-8qc3`, 2026-09-14T02:26Z).

**The question.** A heap-use-after-free in qpdf 12.4.1 is reachable from opening any untrusted
document (#62). What of ours can an attacker actually reach, and what is worth changing now
rather than waiting for an upstream fix?

## 1. Native: not shipped, so not exposed

The native engines are behind the `native-engines` feature, **off by default**. Nothing built
from this repository today puts a native qpdf in front of a user:

| consumer | reached by | status |
|---|---|---|
| `cargo test --all-features` | a developer, or CI | test-only |
| the fuzz targets | a developer, or CI | test-only, and deliberately fed hostile input |
| the corpus runner | a developer | test-only |
| M3 Android / M4 iOS | a user | **not built yet** |

So the native path's exposure is to inputs we choose, on machines we control, in a process
whose job is to crash on exactly this. That is not a risk; it is the instrument that found the
bug.

**What this does mean:** M3 is the milestone at which this becomes real. An Android app opening
a PDF from a messaging attachment is the scenario, and the ROADMAP's M3 entry now depends on
#62 being resolved or mitigated rather than on it being remembered.

## 2. Web: sandboxed, and the sandbox is the real mitigation

The engines run as Emscripten modules inside a Web Worker. A use-after-free in qpdf is a
use-after-free **within that module's linear memory** — a `WebAssembly.Memory`, an
`ArrayBuffer` the host owns. It cannot:

- read or write outside the module's own heap,
- reach the worker's JavaScript objects, the page, or another origin,
- execute anything: wasm has no writable-executable memory and calls go through a typed
  function table.

What it *can* do is read bytes belonging to something else **in that same heap**. That is the
whole exposure, and it is what the rest of this document is about.

The CSP, the integrity-pinned engine fetches and the worker's own policy guard are unchanged by
this and do nothing about it either way; they govern what code loads, not what a loaded engine
reads from itself.

## 3. What is in that heap alongside the document

One worker serves **many documents** in a session. The host keeps it across operations and
discards it only when a heap crosses the recycling threshold derived from `max_memory_bytes`
(`core/burrow-engines/src/web/recycle.rs`). Someone merging six files, or rotating one and then
another, is using one worker and one pair of engine heaps throughout.

So the question is what the previous document leaves behind.

### 3a. Our own allocations — fixed in this change

The buffer holding the user's file was released with a plain `_free`. That returns the bytes to
the module's free list **with their contents intact**, where they sit until something happens
to allocate over them. Passwords were wiped from the start, on both engines, precisely because
that state is unacceptable; the document — the larger object — had the weaker treatment.

Both engines now wipe it, on the success path and on the refusal path (a document PDFium
rejects reached the heap too, having been copied in before the load was attempted). Two tests
assert the **bytes are zero**, not that the call happened, and re-planting the defect in the
fake fails them.

Cost: one `HEAPU8.fill(0, …)` per document, over bytes about to be released.

### 3b. The engine's own allocations — not fixed, and not fixable by us

qpdf's parsed object cache, its buffers, PDFium's page and font structures: all of that is
allocated by the engine inside its own heap and freed by the engine. `qpdf_cleanup` and
`FPDF_CloseDocument` release it; neither promises to zero it, and we have no seam to make them.

**This is the residue that matters**, and it is the one a use-after-free read could surface.
The realistic shape is: document A is processed; its parsed content remains in freed engine
memory; document B is processed in the same worker and triggers the UAF; the read returns
bytes from A. Both documents are the same user's, in the same session, on their own machine —
so this is not a cross-origin or cross-user leak. It is a within-session one, and for a
privacy-first tool that is still worth removing.

## 4. Recycling the worker between documents — evaluated, not adopted

The mechanism already exists: `Reply::recycle` is computed in Rust, and the host discards the
worker after delivering the result. Making it fire per document is a small change.

**The cost is not small.** A respawn re-fetches 6.8 MB of WebAssembly and recompiles three
modules. The only measurement we have is `apps/web/e2e/measure.spec.ts`, which asserts a median
under **15 seconds** — but it measures against a server sending `cache-control: no-store`
specifically so no HTTP cache can hide the fetch, so that number is a **cold-cache worst case
and not the production figure**. burrow sets no cache headers of its own; what a real deploy
does depends on the host, which means we do not currently know the production respawn cost at
all.

That is the honest state, and it is why this is not being implemented today:

- **It is not cheap on the evidence available.** The one number we have is a worst case an
  order of magnitude beyond an acceptable interaction delay, and the number that would settle
  it — a warm-cache respawn — has never been measured.
- **The residue it removes is now the smaller half.** With §3a fixed, what recycling additionally
  buys is §3b: the engine's own freed structures. Real, and narrower than it was this morning.
- **It trades a certain cost for an uncertain one.** Every multi-document interaction becomes
  slower by a known-large amount, to close a window that requires an unpatched upstream bug to
  exploit and yields the user their own earlier file.

**What would change the decision**, in order:

1. Measure a warm-cache respawn. If it is a few hundred milliseconds, recycle **on a new
   document** — not per operation — and pay it while the user is choosing a file rather than
   during the operation. This is the likely outcome and it is cheap to find out.
2. If upstream does not fix #62 on a timeline we are comfortable with, recycle regardless of
   cost and say so in the page's prose.
3. If a way is found to reset a module's heap without a full respawn — re-instantiating the
   wasm module against a fresh `Memory` while keeping the compiled module — that is strictly
   better than both and is the thing actually worth spiking.

Recorded as follow-up work on #62 rather than done here, because "implement it if cheap" was
the instruction and the measurement says it is not yet known to be cheap.

## 5. What changed today

- The user's document is wiped out of both engine heaps on release, on success and on refusal,
  with tests that assert the bytes.
- The exposure above is written down rather than reasoned about again next time.
- #62 carries the disclosure record; M3 now depends on it.

## 7. Correction, later the same day: `merge` is a different case

§4 concluded that recycling the worker per document addressed "the smaller half". For `merge`
that framing is wrong, and the correction matters more than the conclusion did.

A second qpdf use-after-free was found in the **foreign-object copier**, reached from
`qpdf_add_page` — the call `merge` makes once per page. `merge` copies between documents
**inside a single operation**, so:

- the document wipe (§3a) happens at *release*, after the operation, and does not touch it;
- recycling (§4) happens *between* operations, and does not touch it either.

Neither mitigation in this document applies. That is not an argument for recycling being more
valuable; it is an argument that this class of defect is not addressed by managing what
survives between documents at all.

Measured, on all three paths rather than reasoned about:

| path | outcome |
|---|---|
| native, ordinary release build | deterministic SIGSEGV |
| web (wasm) | hangs; the watchdog kills it at 60 s → typed `LimitExceeded`, worker discarded |
| `qpdf --empty --pages A B --` | completes with warnings; does not reproduce |

Order decides it: the crafted document faults only when it is **not** the first source — your
document first, the one you were sent second.

**Silent corruption was not observed on any path.** What was observed is loud. The reason
`/merge-pdf` is blocked anyway is that "not observed" is not "cannot happen": a use-after-free
read returns whatever is in that memory, and on wasm there is no unmapped page to fault on —
which is precisely why that path hangs instead of crashing. The outcome that would be silent is
the one the platform is most able to produce.

The discharge is #65: `merge` verifies its own output before returning it. That converts the
only unbounded outcome — a plausible-looking wrong document — into a refusal, and leaves the
crash and the hang bounded by the process boundary and the watchdog respectively.

## 6. What has not changed

- No mitigation is claimed for §3b. The engine's own freed memory is still there.
- The wasm sandbox is doing the heavy lifting, and it is not ours — it is the platform's.
- Nothing here reduces the case for the upstream fix.
