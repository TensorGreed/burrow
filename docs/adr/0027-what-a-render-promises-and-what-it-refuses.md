# 0027. What a render promises, what it refuses, and the three numbers that were chosen

Date: 2026-09-16

## Status

Accepted — closes the ceiling [ADR 0020](0020-rotate-ships-without-thumbnails.md) deferred, and
constructs [`Stage::Pixels`](../../core/burrow-types/src/stage.rs) for the first time.

**Amended 2026-09-17 twice.** First: progressive render is required, and is a precondition for
the thumbnail strip rather than an improvement on it — §2a's closing paragraph, which files it
as an open decision, is superseded there. Second: it was **measured**, it passed, and what it
does *not* cover turned out to be most of the cost. Both amendments are at the end.

---

## READ THIS FIRST: three numbers here were CHOSEN, not measured

**Every other limit in this repository was derived from something we observed.** `maxDurationMs`
is 12 s because `e2e/measure.spec.ts` timed the slowest honest operation at 611 ms. ADR 0015's
512 MiB recycle threshold came from watching engine heaps on the corpus. ADR 0018's 240 s came
from `pdfium.wasm` at 5.3 MB over a throttled link. The house style is that a number with no
measurement behind it is a number somebody will over-trust, and `CLAUDE.md` makes an overclaiming
doc comment a bug rather than a wording preference.

**These three are not of that kind, and this section exists so a future reader cannot mistake
them for it.**

| | value | how it was arrived at |
|---|---|---|
| the render ceiling a page asks for | **4 Mpx** | arithmetic over page geometry, then judgement |
| the device-pixel-ratio cap for thumbnails | **2** | judgement: sharpness against 4x the bytes |
| the live-thumbnail window | **112** | arithmetic over viewport sizes, then judgement — corrected twice by computation (§3) |

**No number in this ADR was measured on a phone when it was written, and one observation on one
phone has happened since — [Amendment 3](#amendment-3-2026-09-17--one-device-one-observation).**
The paragraph below is kept as it was written, because what it says about *provenance* is still
true: these are policy numbers, and a single device that survives them does not turn them into
measurements. What changed is that they are no longer supported by arithmetic alone.

They were computed on a desktop from page
geometry and bytes-per-pixel, and the only device evidence behind them is
[ADR 0015](0015-web-worker-lifecycle.md) §7's still-open observation that *on iOS a memory spike
kills the whole tab rather than the worker*. §7's device question was open when this was written
and is **discharged for the thumbnail strip, and only for it**, by Amendment 3. **What settles
these numbers is that test, and this ADR is written to be revised by it** — see *[The three
revision points](#the-three-revision-points)*, which names each constant, where it lives, and
what evidence would move it.

The reason they are chosen at all is that they answer a question measurement cannot: not *how
much can this machine do*, but *how much should a page ask a stranger's phone for*. That is a
policy, and a policy stated with a fake provenance is worse than one stated as a choice.

---

## Context

ADR 0020 deferred thumbnails for one reason, and it was not build cost — it measured that at
near zero. It was **a class of resource ceiling that does not exist yet**: a thumbnail at 120x160
is 76,800 B, `Limits::DEFAULT.max_pages` is 10,000, and a full strip is therefore **768 MB** held
in the engine heap, copied across the bridge, and held again on the main thread. `max_memory_bytes`
**detects rather than bounds** (ADR 0007's 2026-09-12 amendment), so it would report that overrun
after the allocation that caused it.

Two things were true of the limit set when this work began, and both are defects rather than gaps:

- **`Stage::Pixels` is documented "Reserved. Nothing constructs this yet."** It was added so the
  enum described the limit set rather than today's subset of it. Render is its first constructor.
- **`max_pixels`' rustdoc promises, in the present tense, a check that does not exist**: *"Exact:
  checked against declared dimensions before any raster is allocated."* Nothing in the codebase
  decoded a raster, so nothing checked it. That sentence becomes true in this change rather than
  being softened.

### The finding that decided the number

`Limits::DEFAULT.max_pixels` is 256 Mpx — exactly 1 GiB at 4 B/px, and exactly
`max_memory_bytes`. Against real page geometry:

| | pixels | Mpx | MiB at 4 B/px |
|---|---|--:|--:|
| A4 at 72 dpi, 1:1 | 595x842 | 0.50 | 1.9 |
| A4 at 150 dpi — screen reading | 1240x1754 | 2.17 | 8.3 |
| A4 at 200 dpi | 1653x2339 | 3.87 | 14.7 |
| US Letter at 200 dpi | 1700x2200 | 3.74 | 14.3 |
| A4 at 300 dpi — print | 2479x3508 | 8.70 | 33.2 |
| A0 at 72 dpi, 1:1 | 2384x3370 | 8.03 | 30.6 |
| **the PDF maximum page, 14400x14400 pt, at 1:1** | 14400x14400 | **207.36** | **791.0** |
| thumbnail 120x160 CSS at DPR 1 | 120x160 | 0.019 | 0.073 |
| thumbnail 120x160 CSS at DPR 2 | 240x320 | 0.077 | 0.29 |

**A maximum-size PDF page rendered 1:1 costs 791 MiB and passes the 256 Mpx default.** So
enforcing `max_pixels` at its default would enforce nothing that matters on the platform where it
matters most. The default is a **caller** ceiling — the outer bound on any raster this core will
produce for anyone — and it is not, on its own, a device ceiling.

## Decision

768 MB had three independent causes, so this is three ceilings rather than one number.

### 1. `max_pixels` becomes real, and the page asks for 4 Mpx

**Checked in Rust, against `width x height`, before `FPDFBitmap_Create` — not after it returns
null.** The failure is `Error::LimitExceeded { limit: "max_pixels", stage: Stage::Pixels,
requested, allowed }`: typed, raised before any allocation, and an **ordinary outcome**. Per
`apps/web/CLAUDE.md`'s worker contract it must not cost a worker and cannot latch the circuit
breaker; only `Internal` does that.

`Limits::DEFAULT.max_pixels` **stays at 256 Mpx.** Lowering it would change native and mobile
behaviour to settle a question about a browser tab, and it is the right shape for what it is: the
bound on a hostile render *request*, not a thumbnail policy.

What changes is what the web page asks for. `LIMITS.maxPixels` in
`apps/web/src/components/tool-host.ts` drops from 256 Mpx to **4,194,304 (2048², 16 MiB per
bitmap)**. In practice:

- **a thumbnail** at 120x160 CSS px, DPR 2, is 0.077 Mpx — **1/54th of the ceiling**;
- **a full page at reading resolution** — A4 at 200 dpi, 3.87 Mpx — fits, with room; at 150 dpi it
  fits twice over;
- **what it refuses**: a full page at 300 dpi (8.70 Mpx), and any 1:1 render of A0 or larger.
  Those are print resolutions and this is a screen.

**Two places check it, and the outer one subsumes the inner.** `burrow_ops::render::begin`
checks the **box** before the document is opened, so a caller asking for something impossible
is told so without an untrusted file having been parsed on their behalf. The engine checks
each **page's raster** immediately before `FPDFBitmap_Create`. A page fitted into a box never
has more pixels than the box, so on the strip path the inner check can never fire — it is kept
because the engine is also reachable by callers that ask for an exact size, and because a check
that guards an allocation has to sit next to it. Said here rather than left for somebody to
find by instrumenting it.

**The aspect ratio is never the document's choice.** The caller states a target size and PDFium
scales the page into it, so a 14400-pt page at thumbnail size costs the same 0.29 MiB as any
other. The ceiling therefore guards *our own code and other callers of the core*, not a person: a
visitor cannot reach it through the thumbnail strip, and that is the intended relationship between
the two.

### 2. One render in flight — structural, not a number

ADR 0015 §2a already serialises the worker, and ADR 0009 §2 requires Rust to orchestrate the
loop. One bridge call renders one page and frees its bitmap before returning, so **the engine
holds at most one bitmap** — 16 MiB at the ceiling, 0.29 MiB in practice.

**ADR 0020's 768 MB — ten thousand thumbnails held at once — is not bounded by a number here;
it is unreachable by any code path.** That is a stronger statement and it is the one worth
having, because a number can be raised by an edit that looks local while a shape cannot.

### 2a. What is bounded, and what is NOT

**THIS SECTION SAID "the engine heap holds at most one bitmap" AND THAT WAS AN OVERCLAIM.** It
is true of the *bitmap*. It is false of the heap, and a security review measured how false,
against the real `libpdfium.so`, rendering a **thumbnail** — 240×320, one fifty-fourth of the
ceiling above — under the web app's own limits:

| input | content | one engine call | peak RSS |
|--:|---|--:|--:|
| 403 kB | 3 M stroked paths, one content stream | **34.0 s** | **819 MB** |
| 1.34 MB | 10 M stroked paths | **112.4 s** | **2,717 MB** |

Scaling is linear, so a 13 MB file — well under the 512 MiB `max_input_bytes` default —
projects to roughly nineteen minutes and 27 GB. The same review could not reproduce this
through images: 20000×20000 RGB, 30000×30000 grey and 30000×30000 bilevel all peak at 14–30 MB,
because PDFium decodes flate images scanline-wise into the downsampled destination. **The cost
is the content stream, not the raster and not the image.**

So, stated plainly and in the three places the code says it:

- **`max_pixels` bounds the buffer we hand back.** It does not bound what the rasteriser
  allocates to produce it, and nothing in `Limits` does.
- **`max_duration_ms` is cooperative, and on this path the overshoot is MINUTES.** `Limits`'
  general caveat says "overshoot of up to one engine call". Every other operation's engine call
  is milliseconds; this one's is unbounded, and that deserves its own sentence rather than
  inheriting the general one.
- **`max_memory_bytes` now at least DETECTS here.** `check_measured_memory` runs either side of
  each render on both paths, as it already did at open. It fires after the allocation — that is
  what ADR 0007 says it is — but before the raster is returned, so the operation fails rather
  than handing back something that cost more than the caller allowed.

**What would actually bound it is PDFium's progressive render** — `FPDF_RenderPageBitmap_Start`
and `FPDF_RenderPage_Continue` with an `IFSDK_PAUSE`, which gives the deadline a checkpoint
*inside* the page. That is a design decision with its own failure modes, not a patch, and it is
recorded here as the open item rather than bolted on under review pressure. Until it is taken,
**a single hostile page can hold a worker for minutes and can abort the wasm module** (its 2 GiB
maximum is reached well before the native figures above), which on iOS is the tab-ending spike
ADR 0015 §7 describes.

> **Superseded by the 2026-09-17 amendment.** It is no longer an open decision: it is required,
> and the strip does not ship without it. The paragraph stands as written because a record is
> not rewritten ([ADR 0001](0001-record-architecture-decisions.md)), and because *what it was
> open for, and what closed it*, is the useful part.

**Nothing ships to a visitor on this record's first merge** — the capability merges inert, per
§5 — so this is a bound on what the *next* merge may expose, and the thumbnail strip has to be
built knowing it.

### 3. A live window of 112 thumbnails on the main thread

120x160 CSS px, device pixel ratio capped at **2** — at most 240x320 = 307,200 B RGBA each — and
at most **112 held at once**, about **34.4 MB**, windowed by `IntersectionObserver`: render what
is near the viewport, release what is not. That is **15x** below ADR 0015 §5's 512 MiB recycle
threshold.

**THIS NUMBER WAS 32 WHEN THIS RECORD WAS DRAFTED, AND IT WAS WRONG TWICE — 32, THEN 64.** The
draft argued that *"a 390 px phone shows about 8 of these and a 1280 px desktop about 27, so the
window is never the thing a person notices"*, and that *"9.8 MB sits two orders of magnitude
below"* the recycle threshold. `thumbnail-policy.test.ts` computed both and neither holds:

| claim in the draft | computed |
|---|---|
| a 390 px phone shows ~8 tiles | **18** (3 columns x 6 rows at 844 px tall) |
| a 1280 px desktop shows ~27 tiles | **50** (10 columns x 5 rows at 800 px tall) |
| 9.8 MB is two orders of magnitude below 512 MiB | **55x** below — between one and two |

The second row is the one that mattered: **32 was smaller than a desktop viewport**, so the
window would have blanked tiles while somebody was looking at them — the single thing this
policy must not do. It is recorded here rather than silently corrected because the failure is
instructive: the number was chosen, the sentence justifying it sounded right, and it took
something that *computed* the claim to notice. The test is now the thing that holds the figures,
and the ADR quotes it rather than the other way round.

So the floor is stated as a rule instead of as a number: **the window may not go below what a
viewport shows.**

**64 was written here, and 64 broke the strip.** It cleared a 1280x800 grid and not a 1920x1080
one, which is 112 tiles — and the gap was not cosmetic, which is what this record assumed when
it filed the difference as "a visible cost, on a desktop". A window smaller than the viewport
evicts a tile somebody is looking at, the strip re-requests it, and it never settles: modelled
at 112 wanted tiles against a window of 64, **500 requests and 24,064 renders without
converging**, on a static viewport with nobody scrolling. `CLAUDE.md` calls an unbounded loop a
denial-of-service bug rather than a missing nicety. Both code reviews found it independently.

**Two fixes, because one is the number and the other is the rule.** `strip-schedule.ts` gives a
tile pushed out while still wanted a `released` state that is not re-requested, so **the loop
terminates whatever this constant is** — the number is no longer load-bearing for termination.
And the constant is 112, so on an ordinary desktop nothing is released while somebody is looking
at it. `thumbnail-policy.ts` carries the same history beside the constant, and
`thumbnail-policy.test.ts` computes the figures this section quotes.

**The residual, stated rather than implied.** Above 112 — a 4K grid is about 448 tiles — the
strip degrades to blank tiles beyond the window until you scroll. That is a strip doing less
than it could rather than a strip doing harm, and it is the direction to fail in: this ceiling
exists because of the phone, where about 18 tiles are visible and 112 is six times that.

**This is not ADR 0020's rejected "first N pages only."** That was a *feature* window — pictures
for part of a document and numbers for the rest, with nothing explaining the boundary. This is a
*viewport* window: every page gets a thumbnail when you look at it. The two look alike from a
distance and differ in the only way that matters to the person using it.

### 4. Render is a read, and ADR 0022 gains no variant

[ADR 0022](0022-every-operation-verifies-its-own-output.md) requires every operation to verify its
own output through a fresh engine. **`verify::Expected` gains no variant for render, and that is a
decision rather than an omission.** Rendering returns pixels, not a document; there is nothing to
reopen. It is a read, like `page_count` and `page_rotations`.

What *is* checked, because it is free and because the bitmap's length is a **decision**
(`w x h x 4`) rather than something the file supplied:

- the requested page index was in range before the call;
- the returned buffer's length equals `width x height x 4`, asserted rather than trusted.

**What is not checkable, written down rather than left implied: that the pixels are of the page
that was asked for.** Nothing in the returned buffer identifies its source page. The conformance
observable in §5 is what would catch a systematic off-by-one across a corpus; it would not catch a
single wrong page in a single call. Stated per the `add-operation` rule that a promise with no
stated residue is one somebody over-trusts.

### 5. The conformance observable is not a pixel hash

**It lands in the second merge, and that is stated rather than left as an assumption.** The
capability merges first, inert: no page renders, so the render bundle reaches no visitor — the
same shape [ADR 0026](0026-how-rendering-loads-without-returning-to-the-old-payload.md)'s own
merge had, and it is confirmed before merging rather than assumed. The differential case needs
the harness to drive the *render* worker from a page, which is the wiring the thumbnail strip
builds; putting it in the first merge would mean building that wiring twice.

**The grid's arithmetic is pinned now, on the native side only**, so the readout is not being
designed while two platforms are already disagreeing about it:
`core/burrow-ops/tests/render.rs` carries the golden grid for a one-quadrant fixture and a test
that the grid moves when a page is turned — which is the property that makes it an observable
rather than a checksum.


Native links `libpdfium.so` and the web loads `pdfium.wasm`, both from `pdfium-binaries` at
`chromium/8044` — the same rasteriser on different targets, so antialiasing may differ without
anything being wrong. `Outcome::Ok` gains `render: Option<{ width, height, ink_grid: [u8; 16] }>`:
a 4x4 downsampled luminance grid quantised to four levels. Dimensions alone cannot see a wrong
page; a single `ink: bool` cannot see a rotation, which is the case `/rotate-pdf` exists for.

**If the grid diverges between platforms it is recorded in `platform_expectations` with a reason,
or narrowed to dimensions plus ink — measured, not assumed.** Whichever way it lands, the residue
is stated rather than the assertion quietly weakened.

## The three revision points

**Each number lives in exactly one named place, so revising it is one edit rather than a search.**

| # | constant | file | today | what would move it |
|---|---|---|--:|---|
| 1 | `LIMITS.maxPixels` | `apps/web/src/components/tool-host.ts` | 4,194,304 | a 16 MiB bitmap spiking a real iPhone tab → lower it. A future "view this page properly" feature needing 300 dpi → raise it to 8,704,000, at 34 MiB per bitmap. |
| 2 | `LIVE_THUMBNAIL_WINDOW` | `apps/web/src/components/thumbnail-policy.ts` | 112 | 34.4 MB proving too much resident on a phone → lower it, but **not below what a viewport shows** (18 tiles on a 390 px phone, 50 on a 1280 px desktop, 112 on a 1920x1080 one — computed in `thumbnail-policy.test.ts`). A 370-page document held at this window on an 8 GB iPhone without the tab dying — Amendment 3 — so the evidence that moves it downwards is now a *smaller* phone, not this one. |
| 3 | `MAX_DEVICE_PIXEL_RATIO` | `apps/web/src/components/thumbnail-policy.ts` | 2 | DPR-2 thumbnails costing more than the sharpness is worth → 1, which **quarters** each thumbnail to 76,800 B and the window to 4.9 MB. The largest single lever here, and the one to pull first. |

**The measurement that settles all three is ADR 0015 §7's deferred device test**: render a strip
on a real iPhone and watch for tab termination, because on iOS a memory spike kills the tab rather
than the worker. **That test has now been run once, on one device** — Amendment 3 — which is why
the middle column of row 2 moved and why the "what would move it" column now names a smaller
phone. It is one observation, not a range, so the honest description of these numbers is still
the one at the top of this record.

`thumbnail-policy.ts` exists as its own module for this reason and not for tidiness: a policy
spread across two islands is a policy nobody can revise with confidence, and these are the numbers
most likely to be revised.

## Consequences

**The `max_pixels` rustdoc becomes true.** It promised an exact check in the present tense for the
whole of M1. `Stage::Pixels`' "Reserved" paragraph is replaced by what constructs it.

**A refusal a person can hit is a refusal a person must understand.** They cannot hit it through
the strip — §1 — so the message is for a caller, and the page's message function still owes it a
sentence for the same reason every other typed error has one.

**Every figure in this record is computed by a test, not asserted in prose.**
`apps/web/src/components/thumbnail-policy.test.ts` holds the tile size, the window bytes, the
viewport counts and the relationship to `LIMITS.maxPixels`; `core/burrow-engines/src/raster.rs`
holds the page-geometry table's decisive rows. That is a direct consequence of §3: the one claim
here that nothing computed was the one that was wrong.

**Two workers can be alive at once and nothing bounds their sum.** `apps/web/CLAUDE.md` already
records this as an open consequence of ADR 0026; render adds the second heap that makes it
concrete, and it is a third input to ADR 0015 §7 rather than something this record closes.

**The uniffi binding is NOT updated, and that is deferred rather than missed.** `burrow-core`'s
surface gained `Fit`, `Render`, `Rendered`, `render_begin` and `render`; `bindings/burrow-wasm`
exposes them and `bindings/burrow-ffi` does not. The definition of done allows either
"reflected in bindings (uniffi + wasm)" **or** "explicitly deferred", and this is the second:
M3 and M4 render with the platform's own rasteriser (`ROADMAP`), so a uniffi `render` would be
a surface neither app is going to call. If that changes, it is a decision with its own record.

**`apps/web/src/components/thumbnail-policy.ts` has no consumer yet**, and lands with this
merge rather than with the strip on purpose: it is where this record's revision points live, and
a record that names a file nobody has written is a record nobody can act on. Its test is what
holds the figures quoted here.

**These numbers will look arbitrary to somebody in a year, and that is the correct reading.** They
are arbitrary in the specific sense of being chosen rather than derived, they are documented as
such, and the table above tells that reader exactly which three edits to make once they have the
device in front of them.

---

## Amendment, 2026-09-17 — progressive render is required

**Approved by the repository owner, who asked for the reasoning to be recorded here rather than
in a commit message.** Their argument, in their terms:

> *without it a hostile page's only outcome is a dead worker, which on iOS is a dead tab, and
> progressive render turns it into a typed per-tile refusal — the functions are already exported
> and the pause callback keeps every decision in Rust.*

That is the whole of it, and each clause is load-bearing.

### What changed between §2a and this amendment

§2a filed progressive render as an open decision because it looked expensive: a new engine
surface, and a callback from C++ into our own code in the middle of an engine call. Three
findings moved it, and all three arrived after §2a was written.

**1. It costs no engine rebuild.** `FPDF_RenderPageBitmap_Start`, `FPDF_RenderPage_Continue` and
`FPDF_RenderPage_Close` are all exported by the vendored `pdfium.wasm` **and** by
`libpdfium.so` — checked, not assumed. That is the same finding [ADR 0020](0020-rotate-ships-without-thumbnails.md)
made about the render calls themselves: no `EXPORTED_FUNCTIONS` allowlist, so no export-list
change, no engine rebuild, no new content hash, no CSP regeneration, no size-budget re-measure.
Three of §2a's six rows cost nothing here for the second time.

**2. The pause callback does not put a decision in JavaScript.** The obvious reading of
`IFSDK_PAUSE` is a callback that decides when to stop, which [ADR 0009](0009-web-panic-contract-and-binding-boundary.md)
§2 forbids. It does not have to be. `NeedToPauseNow` is a **constant that always returns true**,
so PDFium completes one slice and returns, and the loop is Rust's:

```text
status = Start(bitmap, page, …, always_pause)
while status == FPDF_RENDER_TOBECONTINUED:
    deadline.checkpoint()?          ← Rust
    check heap growth               ← Rust
    status = Continue(page, always_pause)
Close(page)
```

The callback is not a branch on engine state; it is a constant function with no allocation and
no panic path. Every decision — continue, abandon, refuse — stays where the native path runs the
same code, which is the property ADR 0009 exists to keep.

**3. It is the only thing that makes "a slow page does not stop the rest" true.** This is the
clause that decided it. Without a checkpoint inside the page, the *only* mechanism that can end
a runaway render is the host's watchdog, and `worker-host.js`'s `watchdogFired` calls
`discard(…, { crash: true })`: the worker is terminated, the whole strip fails, and it counts
against the circuit breaker, so three hostile pages in sixty seconds take the renderer offline.
With the checkpoint, a hostile page becomes an ordinary `Error::LimitExceeded` for **that tile**
— the session survives, the worker survives, the breaker does not count it, and page 8 renders.

### The decision

**Progressive render lands before the thumbnail strip. It is a precondition for any rendering
UI, not an improvement on one.** A strip built on terminate-only isolation is a strip whose
worst case is a dead tab on the device this whole record is about.

Two things land with it:

- **A measured per-page render budget.** `LIMITS.maxDurationMs` is 12 s because
  `e2e/measure.spec.ts` timed the slowest honest *document* operation at 611 ms. A thumbnail is
  a different workload and 12 s was not known to be its number. `measure.spec.ts` gains render
  timings and the budget is derived the same way. **Unlike the three numbers at the top of this
  record, that one will be measured rather than chosen** — and it was expected to be the
  cheapest lever, because the spike is allocation rate times budget.

  > **It was measured, and it is NOT that lever.** The slowest honest page render in the corpus
  > is `objstm-bomb.pdf` at about **1.2 s** — roughly 124x the next slowest fixture, and
  > **twice** the 611 ms the document budget was derived from. A render budget tight enough to
  > shrink the spike meaningfully would refuse a document that is in the corpus on a device four
  > times slower than the runner, and a premature refusal is a wrong answer where a slow one is
  > merely slow. So **the two workloads share the 12 s budget**, which is now a measured
  > statement rather than an inherited one, and the paragraph above is left standing with this
  > correction beside it because what it expected is as useful as what it found.
  >
  > The finding only exists because the first version of that measurement asked every fixture
  > for pages 1–4, and `render` refuses the whole request when a page is past the end — so
  > `blank-1page.pdf` and `objstm-bomb.pdf` drew nothing and were silently dropped from the
  > sample. It reported 10 ms. **The fixture that decides this number was the one that fell
  > out**, and the test now asserts its sample is the whole fixture list rather than "at least
  > one".
- **A heap check between slices.** `check_measured_memory` currently runs either side of a whole
  render. Between slices it can *stop* a growing render rather than report on a finished one.
  It still **detects** rather than bounds: the megabyte that crossed the line has already been
  allocated, and the reading is taken every sixteenth slice rather than continuously. What it
  buys is that the overrun ends the render instead of being reported after it.

  > **This paragraph said "bounds instead of detects" and that was wrong.** `core/CLAUDE.md`
  > singles the word out — say "detect" where we detect and "bound" only where something is
  > actually bounded — and records that an overclaim of exactly this kind was load-bearing in
  > two later ADRs' reasoning before it was measured. Nothing here bounds `max_memory_bytes`;
  > ADR 0007's amendment still holds on every path. Found by code review.

### What is still unmeasured, said plainly

**PDFium's slice granularity is unknown to us.** `NeedToPauseNow` is consulted at intervals the
engine chooses, not per drawing operation, and if one slice is seconds long then the checkpoint
buys proportionally less. **Measuring it is the first thing the next merge does**, before any of
the design above is relied on — because a checkpoint that fires rarely is a bound in the same
way a 12-second budget is a bound, which is to say barely.

And the bound this buys is still on **observed** growth: memory can grow inside one slice with
nothing watching. That is weaker than a real ceiling and stronger than anything on this path
today, and it is written down in those terms rather than as a solved problem.

---

## Amendment 2, 2026-09-17 — measured, and what it does not cover

The first amendment made progressive render required on the strength of an argument. This one
is the measurement, taken against **the acceptance bar written down before the harness existed**
— because a bar set after the numbers is not a bar.

### The bar, pre-registered

| verdict | condition |
|---|---|
| PASS | longest slice ≤ 500 ms and p99 ≤ 250 ms — at least as fine as `WATCHDOG_GRACE_MS` |
| MARGINAL | 500 ms < longest ≤ 1000 ms — a deadline mechanism, not a cancel one |
| FAIL | longest > 1000 ms — stop; the design changes rather than the UI absorbing it |

Plus per-slice memory growth (PASS ≤ 32 MiB, FAIL > 128 MiB), and an explicit null result: **if
`_Start` returns `DONE` on the first call the answer is FAIL however fast it was**, because a
mechanism that does not pause is not a pause mechanism. That is the outcome most likely to read
as success, since the render still completes.

### The numbers

The two adversarial inputs from [#103](https://github.com/TensorGreed/burrow/issues/103) — 3 M
and 10 M stroked paths in one flate content stream, 342 kB and 1.14 MB — drawn at 240x320.

| | 3 M paths | 10 M paths |
|---|--:|--:|
| slices | **30,002** | **100,002** |
| slice p50 / p99 | 0.3 / 0.4 ms | 0.4 / 0.4 ms |
| **longest slice** | **0.5 ms** | **0.7 ms** |
| worst per-slice growth | 0.7 MiB | 1.0 MiB |

**PASS on both bars, by three orders of magnitude on time.** PDFium yields about every hundred
paths. `SLICES_PER_MEMORY_READING` is 16 because of the right-hand column: sixteen slices is at
most 16 MiB between heap readings.

### `FPDF_LoadPage` is uninterruptible, and it is most of the cost

| | load phase | peak |
|---|--:|--:|
| 3 M paths | **757 MiB in 1,166 ms** | 767 MiB |
| 10 M paths | **1,765 MiB in 3,511 ms** | 2,537 MiB |

**Interruptibility here is a property of rendering, not of loading.** Around 70% of the peak is
allocated inside one call that has no checkpoint and no progressive variant. Progressive render
does not shrink that; it shrinks everything after it — 40 s to 3.5 s, 2,537 MiB to 1,765 MiB.

Four things follow, and they are constraints on the strip rather than caveats on this record.

**1. What happens when the per-page deadline is exceeded during the load: the worker dies.**
The Rust checkpoints cannot fire — there is nowhere to put one — so the page's watchdog fires at
`budgetMs + WATCHDOG_GRACE_MS` while the worker is inside `FPDF_LoadPage`, and
`worker-host.js` terminates it and counts a crash. Nothing can make that a typed refusal.

So **the strip is driven as a sequence of requests, not as one**. A worker death costs the
request in flight; the tiles already delivered stand, because the host hands each page over as
it arrives rather than holding them the way it holds a split's parts. The page re-requests the
pages after the one that died — **the page that killed the worker is marked, never retried**,
which keeps `apps/web/CLAUDE.md`'s "never retry automatically". Three such pages in sixty
seconds latch the render breaker, and the page surfaces that as something a person can act on.

**What the person sees is the tool still working.** The strip is an aid: `/split-pdf` and
`/rotate-pdf` select by page number and shipped without pictures at all (ADR 0020). A tile that
cannot be drawn keeps its number and says so; it does not become an error banner, because
nothing the person did is wrong and nothing they need is unavailable.

**2. One page load in flight, and the next does not start until the previous is gone.** The
first half is structural — the engine thread and the worker are both serialised, so a second
`FPDF_LoadPage` cannot begin while one is running. The second half is
`estimate::before_page_load`: since the load cannot be checkpointed once entered, the only lever
on it is the decision to enter, so a strip that has already spent `max_memory_bytes` since the
document was opened is refused **before** the next load rather than after it.

On the web "the previous has freed" is not observable at all — the module's heap never shrinks.
What makes it true there is the worker being replaced: each page's reply carries Rust's
`recycle` verdict, and the strip now **stops on it mid-strip** and says so, rather than leaving
the verdict to a terminal reply that carries none. The caller re-requests the rest into a fresh
worker.

**3. The pre-scan cannot cheaply refuse these files, and that is evidence for #24 from a second
direction.** The obvious move is to refuse before loading, using what the file declares — and
the pre-scan already reads `Declared::largest_stream`. It does not work, for a specific reason
worth writing down: **that is the COMPRESSED `/Length`**. The 3 M-path fixture is 342 kB of file
declaring 64 MB of drawing, a 187x amplification the declaration does not mention. Refusing on
compressed length would fire on any image-heavy page, where a large stream is ordinary and cheap
to skip.

Modelling inflation needs an inflater, and there is **no inflate dependency anywhere in this
workspace** — so it is a dependency decision and a licence audit, which is
[#24](https://github.com/TensorGreed/burrow/issues/24)'s own batch and not a thumbnail strip's.
**#24 stays open.** If the load-phase residual turns out to need a bound before the strip can
ship, that is a decision to bring back rather than an inflater to reach for mid-change.

**4. Cancel was never the thing this improved, and the first amendment implied otherwise.** A
Stop gesture is `discardWorker()`, which terminates — immediate, and always was. A worker cannot
observe a message mid-operation anyway: its message loop is not running while wasm is. What
progressive render changed is the **deadline**: it now ends a runaway render without costing the
worker, where before the only mechanism was the watchdog killing it. Stated plainly because the
earlier framing — "a Stop that takes up to a second is a Stop somebody presses twice" — argued
from the wrong mechanism.

---

## Amendment 3, 2026-09-17 — one device, one observation

**The numbers at the top of this record are no longer supported by arithmetic alone.** They are
supported by arithmetic and by **one observation on one phone**, which is a different thing from
being measured and a different thing again from being validated across a range.

### What was observed

| | |
|---|---|
| device | **iPhone 16 Pro** (8 GB) |
| OS | iOS — *exact version not captured at the time of the observation; to be filled in* |
| page | `/split-pdf` on the deployed site |
| document | **11 MB, 370 pages** |
| what was done | opened it and **scrolled the strip to the end** |
| outcome | **no tab termination**; thumbnails visually correct throughout |

Scrolling a 370-page strip to the end is the part that makes this worth recording. The window
holds 112 tiles, so reaching the end means roughly 370 renders and about 258 evictions, with the
main thread repeatedly at a full window of **34.4 MB** of RGBA while the render worker held one
bitmap and the document. That is the shape [ADR 0015](0015-web-worker-lifecycle.md) §7 warned
about — a memory spike on iOS takes the tab, not the worker — exercised for the length of a long
document rather than for one page.

### What it does and does not support

**Directly.** `LIVE_THUMBNAIL_WINDOW = 112` and `MAX_DEVICE_PIXEL_RATIO = 2` decide what is
resident on the main thread, and residency across 370 pages is exactly what was exercised. The
eviction path was exercised too, which the livelock in §3 shows is not the same as the happy one.

**Indirectly, and this is worth being exact about.** `LIMITS.maxPixels = 4 Mpx` was never
approached: a 240x320 thumbnail is 1/54th of it, and §1 is the argument that a person cannot
reach the ceiling through the strip. What this observation supports is that **the ceiling was
not in the way** — a strip drawn under it worked on a real phone. It says nothing about whether
4 Mpx is the right ceiling for a caller that asks for a full-size render, because nothing here
asked for one.

**Not at all.** The 8 GB in the table is the reason this is one observation rather than a range.
An iPhone 16 Pro is at the **generous end** of the devices this has to work on; a 4 GB phone —
an iPhone SE, a mid-range Android — is **untested**, and the memory available to a tab there is
not 8 GB minus the same overhead. **So the numbers stay conservative.** One device surviving is
a reason not to lower them in a panic; it is not a reason to raise any of them.

### What would still move these numbers

- **A 4 GB device terminating the tab, or visibly thrashing, on the same document.** That lowers
  `MAX_DEVICE_PIXEL_RATIO` to 1 first — the largest single lever, quartering each thumbnail to
  76,800 B and the window to 8.6 MB — before `LIVE_THUMBNAIL_WINDOW` is touched, because the
  window has the viewport floor under it and DPR does not.
- **A device whose viewport wants more than 112 tiles.** The floor rule wins over the ceiling:
  the window may not go below what a viewport shows.
- **A caller that renders at full size rather than thumbnail size**, which is the only thing that
  would put 4 Mpx under real pressure. There is no such caller today.
- Nothing here moves the pixel ceiling upward. That needs a feature that wants 300 dpi and a
  measurement of a 34 MiB bitmap on a phone, neither of which exists.

### ADR 0015 §7: discharged for this case, and what stays open

§7 deferred per-platform `Limits` defaults for mobile browsers to "a decision with the numbers in
front of it". **For the thumbnail strip that question is now answered: no mobile-specific default
is needed, because the strip's defaults survived a long document on a real iOS device.** That is
the discharge, and it is narrow on purpose.

**What remains open, unchanged by this observation:**

1. **§7's actual exposure is not the strip.** It is *any caller that raises `max_memory_bytes`
   above what the pre-scan would refuse* — a 330 kB file driving PDFium to 1.9 GiB in a worker.
   The strip never raises it. Nothing here touches that path, and an iOS tab dying on it remains
   exactly as possible as it was.
2. **The document worker is not the render worker.** This observation covers rendering. `merge`,
   `split` and `compress` run in the other bundle, against qpdf, with their own peaks.
3. **The load phase is still the unbounded part.** Amendment 2 measured `FPDF_LoadPage` at
   1,765 MiB inside one uninterruptible call on an adversarial file. A 370-page ordinary
   document does not exercise that, and a hostile page on a 4 GB phone is the case §7 describes
   and this does not cover.
4. **One device is not a range**, and a second observation on a 4 GB phone is worth more than ten
   more on this one.

**This amendment does not close §7.** It removes the strip from §7's list of unanswered
questions and leaves the list.

### Correction, 2026-09-17, later the same day: the discharge above claims too much

**It says the strip's defaults "survived a long document on a real iOS device" and concludes no
mobile-specific default is needed. Desktop WebKit says otherwise**, and the measurement is
[#107](https://github.com/TensorGreed/burrow/issues/107): on `/split-pdf` and `/rotate-pdf` — the
two pages that mount this strip — a WebKit tab is lost, or an operation's output never arrives,
about **four times in 580 runs**. With the strip suppressed on the same commit, and on the commit
before it existed, **zero in 580**. Only the strip-bearing pages have ever failed.

So the mechanism §7 warned about — a memory spike taking the whole tab — is reachable with this
strip on a desktop browser.

**Corrected 2026-09-18: the mechanism is PDFium resident *while an operation runs*, not "a tab
holding two engines".** The documents worker persists after an operation, so qpdf and PDFium are
both resident whenever this strip draws at all — before a split and again afterwards. The
withdrawal appeared to show otherwise only because a withdrawn strip never ran. What separates
the failing case is an operation running while PDFium is still there, which is the window the
fix in #107 removes.
One iPhone surviving one document did not establish that it was not; it established that one
device survived one document, which is what this record should have said and what the section
above now has to be read against.

**What stands:** the observation itself, and everything in *What it does and does not support*.
**What does not:** "for the thumbnail strip that question is now answered". It is not answered.
The strip is back on §7's list, and #107 is where the answer will come from.

**The pages rework ([ADR 0028](0028-an-accent-colour-and-what-it-cost-the-reserved-palette.md))
left the strip alone**, deliberately: it works and it looks right, so that change preserves its
behaviour rather than redesigning it. The observation above therefore still describes what
ships.
