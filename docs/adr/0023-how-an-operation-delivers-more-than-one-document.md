# 0023. How an operation delivers more than one document

Date: 2026-09-14

## Status

Accepted.

## Context

`split` is the first operation whose output is more than one document, and the reply protocol
cannot express that. `bindings/burrow-wasm`'s `Reply` carries a single `Vec<u8>`; the worker posts
exactly one message per request; the host's lifecycle ([ADR 0015](0015-web-worker-lifecycle.md))
is a state machine with one terminal reply. Every one-in-one-out operation — `merge`, `rotate`,
`reorder`, and `compress` when it lands — fits that, and `split` does not.

The shape chosen here is inherited by **redaction**, which is why it is an ADR rather than a
field. M2 produces one document, so it could use the existing path unchanged — but it will also
want progress on a long operation, and it is bound by three conditions
([ADR 0006](0006-wasm-linking-strategy.md)'s R8–R10) that a multi-message protocol is precisely
the way to violate. Getting this wrong now is getting redaction's emission contract wrong later.

Four things had to be settled together, because the answer to each constrains the others.

## Decision

### 1. The channel is general: an output arrives with an index and a total

A multi-output operation posts one `part` message per output —

```js
{ id, part: { index, of }, output }     // index is 0-based, `of` is the total
```

— where `output` is a `Blob`, exactly as a single-output reply carries one: structured clone passes
it by reference, so a part never lands in the main thread's own heap. It is followed by the ordinary
terminal `Reply`, which carries no bytes. **There is no
`splitOutputs` field and no operation name anywhere in the protocol.** An operation that produces
one document is the `of: 1` case, and the existing single-value reply remains available for the
four that already use it: this adds a channel, it does not migrate anything.

`index` and `of` are both present on every part, rather than `of` being implied by a terminal
message. A consumer that sees `part 3 of 10` knows what it is receiving without having to have
seen parts 0 to 2, which matters because the host's state machine can be entered by a resumed
page and because "how many are coming" is what a progress bar needs first.

### 2. Parts stream, and the memory that saves is one copy rather than all of them

`burrow_ops::split` returns `Vec<Vec<u8>>`: every part materialised in the wasm heap at once, then
copied again as the worker posts them. A fifty-way split of a large document holds the whole output
**twice**.

So the core grows a session — `split::begin(...)` → `.next_part()` — and the binding exposes it as
a handle the worker **pulls** from. Each part is produced, verified, posted with a transferable,
and dropped before the next is produced.

**Pull rather than push, and that is not a style preference.** A callback would mean Rust code
invoking a JS function mid-operation: a new place for an unwind to cross the boundary, and a branch
on engine state living in JS, which [ADR 0009](0009-web-panic-contract-and-binding-boundary.md) §2
forbids. It also mirrors `PageAssembler`'s begin/append/finish seam, which ADR 0017 chose
deliberately over `merge(inputs) -> bytes` for the same reason.

**What this does and does not buy, stated because the difference is the whole point of measuring
it.** The wasm heap now holds one part at a time instead of all of them. The **host still holds
every part**, because §3 forbids delivering any until all have succeeded. So peak memory falls from
roughly 2× the output total to 1×, and it does not fall to one part.

**What a ONE-WAY split costs, measured, because it is the shape streaming can only lose on.**
Streaming holds the source open while each part is produced and verified; the batching path it
replaced dropped the source before any read-back. With one part there is nothing to win.

`measure-pruning --memory 10000 streaming|batching`, peak RSS growth (`VmHWM`), three runs each:

| | peak RSS growth |
|---|--:|
| streaming (source held across the read-back) | 43.10 MB |
| batching (source dropped before it) | 42.39 MB |

**+1.7%, reproducible to within half a percent.** That is much smaller than the reasoning
predicted — "one whole document worse" was the estimate, and it is wrong: at 10,000 pages the
source's parsed state held across the read-back is ~715 KB of a 43 MB peak, because the peak is
dominated by extracting and writing a part rather than by the source sitting there.

So **a single-part special case is not worth writing.** It would add a second path through the
operation, which is where the leak lived last time, to save 1.7% on the one shape that cannot
benefit from streaming at all.

**One path per process, and the first attempt at this measured nothing.** `VmHWM` is a high-water
mark and never falls, so running both paths in one process had the second measuring the growth the
first had already caused — batching came back at zero and streaming looked like the entire cost.
Confident, and meaningless.

Removing that last copy means writing each part to OPFS as it arrives. That is permitted — R9
forbids writing outside the heap *before verification passes*, and by then each part has passed —
and it is not done here, because it is a storage lifecycle (quota, cleanup on failure, cleanup on
a page the person closes) rather than a protocol. It is the next change if 1× proves too much, and
it is recorded so the reason this stops at 1× is legible.

### 3. A part that fails fails the whole split — the same answer merge gave, for a stronger reason

If part 3 of 10 cannot be produced or cannot be verified, **nothing is delivered**. The host holds
what arrived, discards it, and reports the failure naming the part.

[ADR 0017](0017-merge-engine-and-failure-semantics.md) §2 refused partial success for merge because
"a merged PDF that silently omits an input looks complete", and because a caller who ignores a
per-input outcome list ships a short document and never finds out. The second half applies here
unchanged: every binding, every UI and every future caller would have to handle a per-part outcome
list correctly, forever.

The first half applies **less** well and it is worth saying so rather than borrowing the sentence.
Parts are numbered and delivered as separate files, so nine files where ten were asked for is more
visible than a merged document that is quietly short. Split's case for all-or-nothing is genuinely
weaker than merge's on that axis.

It is stronger on another, and this is the reason that decides it: **`split` is defined as a
partition.** `docs/ROADMAP.md` states the invariant as "every input page appears exactly once
across outputs", and the property tests assert it. Nine parts out of ten is not a partition of
anything — the union is no longer the document. Delivering it would mean the operation's own
invariant holds for no output it produced, which is a different and worse thing than a merge
missing a file.

The cost is accepted and is the same one merge accepts: a fifty-way split that fails on part
forty-nine gives the person nothing. The page is obliged to make the failure legible — which part,
and why — rather than to soften it.

**A failed session stays failed, and that is not the same state as an exhausted one.** The two
look alike from the outside — both drop the source and yield no further part — and the caller's
rule for them is opposite: an exhausted session means deliver the parts you hold, a failed one
means discard them. `SplitSession` therefore keeps the failure and answers with it again rather
than with the empty success that means "every part has been produced". Code review found the
first version unable to tell them apart.

### 4. Each part is verified before it is posted, never the set afterwards

[ADR 0022](0022-every-operation-verifies-its-own-output.md): an operation returns bytes that have
been checked. `next_part()` produces, verifies through a fresh engine, and returns — in that order,
inside one call. **No part leaves the wasm heap unverified**, which is R9 at the part granularity.

Verifying the set afterwards would be strictly worse than the current behaviour, not merely
different: it would mean every part crossing to the host before any of them had been checked, so
the thing R9 exists to prevent would happen ten times before the first check ran.

### 5. R8 is not violated by this, and the argument is narrow

R8 forbids "chunked or progressive emission of partial output" and says "progress may report
*position*; it may not emit *content*". A protocol that emits documents one at a time looks exactly
like the thing that prohibits.

It is not, and the distinction is **what a part is**. R8's hazard is stated precisely: "a trap
mid-stream leaves partially redacted, unverified bytes already in the page's possession, and
terminating the worker does not un-send them." A split part is not a chunk of a document — it is a
complete document, independently verified before it was posted (§4). A trap mid-split leaves the
page holding whole verified documents and no partial one, and §3 means it will not deliver them
anyway.

**So the rule redaction inherits is:** an operation may post `of` parts, and each part must be a
complete output that has passed its own verification. Redaction produces one document, so it uses
this channel with `of: 1` and R8 holds verbatim. **`of > 1` is not available to redaction as a way
to emit a document in pieces**, and `redaction-emission.spec.ts` — R8's named check, still M2's to
write — must assert that the operation produces exactly one part carrying bytes, not merely one
message.

That sentence is the reason this is an ADR. A general channel added for `split` is exactly how R8
would come to be violated by accident two milestones later, by someone who read the protocol and
not the condition.

### 6. Progress is per part, and per part is all the core can honestly report

A progress message names `part` and `of`: one before the first part, so a bar knows how many are
coming before any arrive, and one after each. On a fifty-way split that is **fifty-one** updates; on
a one-way split it is two.

(That count was written as "fifty" and "one" and was wrong in both cases — the leading message was
not counted. Corrected rather than left, because a reader checking the protocol against the code is
exactly who this section is for.)

**Within a part there is nothing to report**, and the protocol does not pretend otherwise. The core
exposes no progress hook inside an operation: the work is engine calls with no callback, which is
the same fact [ADR 0007](0007-limit-enforcement-per-platform.md) records about `max_duration_ms`
being cooperative and checkpointed between outputs rather than inside them. A progress bar that
interpolated inside a part would be inventing a number, and this repository has a rule about
reporting measurements rather than claims.

## Consequences

- **The host holds every part until the split succeeds.** Peak memory is ~1× the output total
  rather than 2×; OPFS-per-part would make it ~1 part, and is the recorded next step.
- **`burrow_ops::split` keeps its `Vec<Vec<u8>>` signature** as a thin loop over the session, so
  native callers, the conformance corpus and every existing test are untouched. The session is the
  primitive; the vector is the convenience.
- **The worker's terminal reply carries no bytes for a multi-output operation.** A consumer that
  reads `reply.bytes` and ignores `part` messages gets nothing rather than the first part, which is
  the direction to fail in.
- **Four operations keep the single-value path.** They are the `of: 1` case and may migrate when
  something else takes them there; two paths is a real cost and is accepted here rather than paid
  as churn across four operations in the change that adds the channel.
- M2 inherits §5 as a condition, not as a suggestion.

## Alternatives considered

**One reply carrying every part.** The shape the protocol already has, extended to an array. It is
the smallest change and it keeps the 2× heap — which is the thing §2 exists to remove — and it makes
the terminal message's size proportional to the output, which is the one message the watchdog is
timing.

**A Rust→JS callback per part.** Genuinely simpler in the worker. Rejected on ADR 0009 §2: a
callback is Rust code calling into JS mid-operation, which is a new unwinding path across the
binding boundary and puts a branch on engine state where the audit surface cannot see it.

**Per-part outcomes, delivering what succeeded.** Rejected in §3, and it is the alternative ADR 0017
rejected for merge with the same second argument. It is friendlier for a fifty-way split of scans,
and it gives up an invariant that cannot be recovered by a caller who was not careful.
