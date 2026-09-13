# 0018. When the engines load, and what bounds a start-up that is slow

Date: 2026-09-13

## Status

Accepted.

## Context

M1 PR B3 left one question open: `/merge-pdf` loads the wasm engines when the first file is
chosen, not when the page loads, and nothing had measured whether that is the right trade. It
is not a question about one page — every tool page inherits the answer — so it was deferred
rather than decided in an island.

Measuring it produced a defect instead of a preference, and the defect has to be dealt with
first, because until it is the comparison is between one option and one that does not work.

### What was measured

A production build served by `e2e/server.mjs`, driven through Chrome DevTools' own network
profiles over CDP, on `/merge-pdf` with one 10-page file.

| | markup readable | island mounted | first file counted |
|---|--:|--:|--:|
| unthrottled | 60 ms | 85 ms | ~0.3 s |
| Fast 4G (9 Mbps, 85 ms) | 351 ms | 368 ms | **6.8 s** |
| Slow 3G (400 kbps, 400 ms) | 2,818 ms | 2,835 ms | **failed** |

The first-load payload is 2.33 MB brotli, of which 2.28 MB is the three engine modules. The
page itself — markup, stylesheets, the typeface and the island — is 41 KB brotli and arrives
in under three seconds on the slowest profile.

### The defect

On Slow 3G the first file did not load slowly. It **failed**, after 66 seconds, with
"Something inside burrow failed" — and the worker was discarded **as a crash**, so three
attempts would have opened the circuit breaker and taken the page offline entirely (ADR 0015
§3). On a slow connection the engines were never going to load at all.

The cause was `DEFAULT_INIT_TIMEOUT_MS`, a 60-second bound on the whole of start-up, carrying
a comment that called it "comfortably above any measured cold load". 6.8 MB at 400 kbps is
over two minutes. The comment had never been measured against a slow connection; it had been
measured against this machine.

## Decision

### 1. The engines load on first use, not on page load

Lazy, as it was — now for a stated reason rather than by default.

The page is readable and interactive in under three seconds on the slowest profile measured,
because it is 41 KB. Loading the engines eagerly would put 2.28 MB of compressed WebAssembly
in front of everybody who lands on the page, including everyone who reads what it does and
leaves. On a metered connection that is somebody's money, spent on a decision they had not
made yet. The whole argument this site makes is that it does not take things from people
without being asked, and 6.8 MB is a thing.

What eager loading would buy is the first file appearing faster for a person who does choose
one, by overlapping the download with the time they spend reading. That is real, and it is
worth less than the cost above, for a reason the measurement makes concrete: on a fast
connection the wait is 7 seconds and nobody needs it optimised, and on a slow one it is 145
seconds, which no amount of overlap hides.

**The cost of this decision is paid visibly rather than hidden.** The island says, while it is
happening, that the engine is being fetched, that it is about 7 MB, and that it happens once
per visit. Showing "counting…" for two and a half minutes with no explanation was the page
being silent about the one thing the person would want to know.

### 2. The start-up bound times silence, not duration

The thing being bounded is somebody else's network, and no fixed number is right for that.
What a watchdog can honestly ask is whether anything is still happening.

So the worker posts `{ starting: true }` as each engine module lands, and the host's start-up
timer starts again each time. A worker that has gone quiet for the bound is hung, which is
what the watchdog is for; one that is still receiving bytes is working, however slowly.

### 3. One message per module is the finest granularity available, and the bound is set from that

Three finer designs were built and measured. Observing the response body through a
`TransformStream`, `tee()`ing it, and reading it with a reader all produced **two** progress
events across a 130-second download of three modules.

The cause is `integrity`. **Subresource Integrity makes the browser verify a whole response
before it releases any of it**, so a module's body arrives in a single read however many
segments carried it — on the same connection, the same file fetched *without* integrity
arrives in 3,545 chunks with a 32 ms maximum gap. Finer progress is available only by giving
up the pinning ADR 0014 exists to keep, so it is not available.

The gap the bound must therefore tolerate is one module's download, and the largest is
`pdfium.wasm` at 5,315,922 bytes. `DEFAULT_INIT_TIMEOUT_MS` is **240 s**, which allows that
module at about 22 KB/s — 177 kbps, below the Slow 3G preset on which the measured gap between
modules is 55 s.

## Consequences

- On Slow 3G the first file now succeeds in ~140 s, where it previously failed at 66 s.
  Measured after the change, not predicted.
- A genuinely hung start-up is noticed in four minutes rather than one. That is the trade: a
  start-up hang is rare and recoverable by reloading, and the failure it replaces made the page
  unusable for everyone on a slow connection.
- `src/host/worker-host.test.ts` holds all three parts against the fake clock: a transfer four
  times the bound survives, a worker that goes quiet is still declared dead, and a `starting`
  message arriving after start-up has settled cannot push anything out.
- **A page-load-time engine fetch is now a decision that needs this ADR amended**, not an
  optimisation someone can add to one tool page. The next four pages inherit the answer.
- The 7 MB figure in the island's copy is rounded from the raw payload, not the compressed one,
  because that is what a person watching a download indicator would see.

## Alternatives considered

**Raise the fixed bound.** Simpler, and wrong in the same way as the original: it encodes an
assumed floor bandwidth without saying so, and the next person on a slower connection meets the
same wall. The bound here still encodes a floor — that is unavoidable — but it is stated, in
bytes per second, beside the module size it was derived from.

**Do not bound start-up at all.** Then a worker that dies silently before acking leaves the
caller waiting forever, which is the hang the watchdog exists to close.

**Load eagerly and accept the payload cost.** Rejected above. Worth revisiting only if the
payload becomes small enough that the question stops being about somebody's data — which would
mean the engines getting an order of magnitude smaller, not a percentage.
