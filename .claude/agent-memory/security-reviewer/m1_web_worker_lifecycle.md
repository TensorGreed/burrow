---
name: m1-web-worker-lifecycle
description: Measured facts about the web worker host (PR 4a-ii / branch feat/web-worker-lifecycle) — where the ack, the pre-ack timer and the breaker actually bind, and what is only guarded by convention.
metadata:
  type: project
---

Verified 2026-09-11 against the working tree of `feat/web-worker-lifecycle` (HEAD 1cc3573
plus uncommitted edits), by driving the real `apps/web/src/host/worker-host.js` with
`fake-worker.js` and an injected virtual clock. Node script, no browser needed — that
harness is the cheapest way to settle any future question about this file.

**The ack is unbounded and the init entry accepts one.** `handleMessage`'s `ack` branch
re-arms `entry.timer` every time it fires and never marks the entry as acked. Measured: 100
re-acks over 100 s of virtual time against `maxDurationMs = 1000` leave the request
unsettled, `state === "busy"`, `terminations === 0`. An ack delivered on the *init*
request's id swaps the 60 s `initTimeoutMs` for `maxDurationMs` and produces
`LimitExceeded/max_duration_ms` for a start-up — the exact inversion ADR 0015 §2 forbids.
Neither is reachable from file content: the bundle acks exactly once, never for `init`, and
its source is SRI-pinned. Both are convention, not structure.

**The pre-ack timer is the sharp edge.** `run()` arms a fixed `initTimeoutMs` timer on every
request, including on an already-initialised `idle` worker, and its expiry is
`discard(..., { crash: true })` — which kills the worker and settles *every other* in-flight
request too. Measured: two concurrent `run()`s, B queued behind A in the worker's single
message loop, both come back `Internal "worker did not accept the operation"` at t=60 s even
though A had acked and carried a 300 s budget. Three of those open the latching breaker.
Nothing serialises `run()`.

**Recycling spawns without bound and that is fine.** 100 replies with `recycle: true` →
100 spawns, `breakerOpen() === false`. One spawn per caller-initiated request; a fresh
worker starts at the module's initial heap, so there is no amplification.

**No engine text reaches the page.** Every `Error::Malformed`/`Error::Internal` payload on
the web path (`core/burrow-engines/src/web/{pdfium,qpdf}.rs`) is a fixed literal, so
`Reply.message = error.to_string()` cannot carry offsets or object numbers; `policy.reason`
is a closed set; `onerror`'s `event.message` is never read. This is the check worth
re-running whenever a new `Error::X(format!(…))` appears on the web path.

**`e2e/server.mjs`: traversal closed, `GET /%` fatal.** `join(root, normalize(path))` plus
the `startsWith` check defeats `..`, `%2e%2e%2f` and `..%2f` (all 404, verified). But
`decodeURIComponent` sits *outside* the `try` inside a `void`ed promise chain, so one
malformed escape is an unhandled rejection and Node exits 1 — the whole e2e run dies.

**`discardWorker()` does not empty the queue, and that is what makes "cancel" partial.**
Verified 2026-09-13 against `apps/web/src/host/worker-host.js` while reviewing
`feat/merge-pdf-page`. `discard()` touches `worker`, `pending`, `generation` and `state`
only; `queue` is a promise chain and survives. A `run()` still waiting its turn therefore
reaches `runOne` *after* the cancel, finds `state === "dead"`, and — since `discardWorker`
passes no `crash: true`, so the breaker still allows it — spawns a **fresh worker** and runs
the operation the user cancelled, to completion. A page whose cancel is `discardWorker()`
plus a generation guard on the reply ignores the answer; it does not stop the work. Nothing
in the host's API cancels a queued request.

**The breaker's `EngineUnavailable` can arrive on a path a page has no gesture for.** It is
returned from `runOne` like any other reply, so a caller that maps replies per-file (a page
counting each input) shows it as a per-file problem, and the "deliberate gesture" that calls
`reset()` is usually wired to a page-level notice. Three fatal `Internal` replies — three
crafted files — are enough to reach that state from file content alone.

Related: [[m1-web-engine-path]], [[m1-limits-real-strength]].
