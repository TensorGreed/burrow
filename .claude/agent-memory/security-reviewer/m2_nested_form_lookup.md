---
name: m2-nested-form-lookup
description: Reviews of m2/164-nested-form-lookup (#164). Round 2 found two /ActualText leaks; round 3 (1ad375f) found two more — the or_insert first-path-wins in scope_of, and a Type 3 font hidden in a form's /Resources — plus the 225x deadline overshoot and a 6/20 mutation survival.
metadata:
  type: project
---

`m2/164-nested-form-lookup`, three review rounds, 2026-09-23. Everything below was **run**.

## The recurring root cause has now produced FOUR leaks in three rounds

*Anything that resolves names differently from the geometry walk is a bypass by construction.*
Rounds 1 and 2 found it in `form_names_for`. Round 3 found it twice more, in the **replacement**.

### Round 3 leak A — `scope_of`'s `found.entry(here).or_insert(inner)` is first-path-wins

For a form with **no `/Resources`**, `inner` is computed against the *enclosing* dictionary, so
it is **path-dependent**. `or_insert` keeps whichever path arrived first. An undrawn decoy form
whose `/XObject` names the shared resource-less form under one scope poisons the name set, and
the span in that form is then checked against names it does not use.

Minimal shape (1.9 kB, `Ok`, canary verbatim in `qpdf --qdf` of the output):
page `/XObject << /A1decoy A /B2real B >>`; `A.res = << /Mid MID /Q W1 >>`,
`B.res = << /Mid MID /Z W2 >>`; `MID` has **no** `/Resources`, stream
`/Q Do  /Span<</ActualText(SEC)>> BDC  /Z Do  EMC`; page draws only `/B2real`. `found[MID]` is
`{"Q"}`, the span wraps `/Z Do`, no refusal. The decoy is never drawn — `scope_of` walks every
`/XObject` entry whether or not the content stream draws it.
**Fix that works (verified, refuses):** `found.entry(here).or_default().extend(inner);`
The depth cap makes `inner` path-dependent for own-`/Resources` forms too, so the union is
needed in general, not only for the inheritance case.

### Round 3 leak B — the Type 3 `Do` refusal only looks at the PAGE's `/Font`

`check_type_three(&resources, &drawn_fonts)` resolves glyph **font names** against
`PageResources` only. Put the Type 3 font in a **form's** `/Resources` and add a decoy `/T3` on
the page pointing at Helvetica: subtype != Type3 → `continue` → the `/CharProcs` are never read.
Measured: 1.7 kB, `Ok`, `BT /Helv 20 Tf 0 0 Td (SECRET) Tj ET` intact in the output.
Without the decoy it is worse in a different direction — `Malformed
"pdf resources [font-missing]"`, which also refuses an **ordinary** document whose form declares
its own font under a name the page does not use (measured on a 5-object fixture).
Both halves are pre-existing on `main`; what is new is that this branch added `Do` to the Type 3
refusal *to close this channel*, and the closure is one `/Resources` level deep.

## What IS closed (all run against 1ad375f)

`/Contents` array with `BDC` in element 0 and `Do` in element 1 (the check runs on
`Contents::concatenate`); `#xx`-escaped resource names (`Token::Name` is decoded and
`top_level_keys` returns decoded keys, so both sides agree); resource-less chains at page level;
`/Pages`-inherited resources; the branch's own five `evade-actualtext-*` fixtures. `category()`
is a plain key lookup — no per-category fallback to diverge from.

## Cost: bounded, but 225x over the deadline, and mostly pre-existing

`form_handle` gets a **fresh** `MAX_FORM_RESOURCE_VISITS = 4096` per call and is called once per
entry in `form_scope` (itself ≤ ~4095), inside `affected_streams`, which has **no deadline
checkpoint in that loop**. Release, `max_duration_ms = 100`:

| file | size | elapsed |
|---|--:|--:|
| 3,900 flat forms, 245-byte names | 2.81 MB | **22.5 s** (41 MB RSS) |
| same on `main` | 2.81 MB | 19.1 s |
| 250 heads x 14-deep chains | 658 kB | 6.7 s (**`main` refuses this in 32 ms** — `form-vanished`) |

So the quadratic is pre-existing; the chain shape is newly reachable because the lookup now
descends. `check_form_sharing` blocks the obvious amplifier: `uses` is **multiplicative** down
the graph (`propagate`), so any many-to-one funnel to a cut form refuses immediately.

## Mutation sweep: 20 planted in `redact_steps.rs`/`geometry.rs`, 20 applied and compiled, **6 survived**

Caught: both `or_insert`/`draws-toward` halves, `scope_of`'s inheritance fallback, its cycle
guard, its `open.remove`, its `/XObject` dict guard, the empty-`wanted` early return, both
`FormsReached::Named` call sites, Type 3 `Do`; and three **hangs** (`scope_of`'s `spend`,
`spend` never refusing, `MAX_FORM_RESOURCE_VISITS = usize::MAX`) — the budget is genuinely
load-bearing for termination on a committed fixture.
Survived: **every ceiling in `walk_forms`** (depth cap, `open.remove`, budget) and — the one
that matters — **`walk_forms`'s inheritance fallback**, which is the headline fix of `133b983`.
Also survived: `scope_of`'s depth cap and its `type_code() != STREAM` guard.

## Harness notes for next time

- Drive it with an example: `core/burrow-engines/examples/sec-redact.rs` (≈30 lines,
  `burrow_ops::redact::page` + `Region`), `--features native-engines --release`. Default region
  for `tools/pdfbuild.py` fixtures is `Region { left: 30, top: 68, width: 340, height: 44 }`
  (page 400x200, `REGION = (30, 88, 370, 132)`).
- A mutation sweep **must restore from a pristine copy read once**, not from the per-iteration
  read: an in-loop restore silently left two mutations stacked, and `cargo test`'s test binary
  **survives `subprocess.run(timeout=)`** — reap `target/debug/deps/redaction_*` by hand or the
  next iteration measures a spinning process.

## Corpus gate (round 3)

Round 2's three findings are fixed and probed by `tools/test-check-redaction-corpus.sh`.
Residual: the self-test mutates the **tracked** `tests/redaction/manifest.toml` in place with
only `trap cleanup EXIT` (no INT/TERM), so an interrupt leaves it modified; the inertness sweep
reports "12 witnesses, none matched" but ~6 of them (`thumb-ink`, `structure-tree-present`,
`image-covers-page`, `vector-fills`, `inline-image`, `image-drawn`) ignore the canary entirely,
so for those it measures "the control lacks that structure", not discrimination; and only
`witness_raw` gets the always-true negative probe. `CARRIER_EVASIONS` in
`redaction_defences.rs` is a hand-listed 6 whose own comment still says "these three".

Related: [[m2_standard14_and_marked_content]], [[m2_qpdf_redact_steps]], [[m2_redact_verify]],
[[spike_0006_redaction_survival]].
