---
name: m2-actualtext-rewriter
description: Reviews of #165's /ActualText marked-content rewriter (d37556c, then cc1d0b9 + be385aa) — the detect/remove asymmetry and its fix, the spans x removals quadratic (109 s from 12 kB), what the post-rewrite check really carries, and the surviving mutations.
metadata:
  type: project
---

Two rounds. Round 1 on `d37556c` (2026-09-24, `/tmp/sec165`); round 2 on `cc1d0b9`+`be385aa`
in a detached worktree. Everything below was **run**. Baselines green both times
(`cargo test --release -p burrow-engines --all-features`, 556 unit + 13 integration suites).

## Round 2: what closed, and what the fix actually rests on

`holds_text_key` was narrowed to **key position** and the difference refused by
`Refusal::MarkedContentCarriesOpaqueString`. Round 1's two leaks
(`/K [ /ActualText (S) ]`, `/Extra << /ActualText (S) >>`) are gone — verified end to end.
28 adversarial shapes tried, **no leak found**.

**But the narrowing is not the load-bearing half.** Mutation M6, widening `holds_text_key`
back to "a name anywhere", **survived the whole suite** — and re-running the 28-shape sweep
under it produced *no leak either*: every shape was caught by the **post-rewrite
`still_names_one` check in `without_carried_keys`**, which decides on the rewritten bytes.
That check is the defence; the narrowing is an optimisation that keeps burrow from blaming
itself. Only mutation M1 (disabling `still_names_one`) turns anything red.

## The quadratic moved, it did not go: 12 kB -> 109 s

Round 1 found `!found.contains(covering)` over a `Vec`; cc1d0b9 replaced it with
`BTreeSet<Span>`. **The set dedups the push, not the scan.** `carrying_spans_over_removals`
still runs `for covering in &open` **once per removal**, so the cost is
`open_spans x removals`. Release, `max_duration_ms = 100`:

| nested carrying BDCs x removed show ops | input | gzip | time |
|---|--:|--:|--:|
| 2k x 2k | 133 kB | 871 B | 0.15 s |
| 8k x 8k | 529 kB | 2.0 kB | 1.45 s |
| 16k x 16k | 1.06 MB | 3.5 kB | 7.00 s |
| **60k x 60k** | 3.96 MB | **11.8 kB** | **109.05 s** (1090x), 417 MB RSS |

Attribution proved by fixing it. Replace `seen` with an amortised cursor: `recorded: usize`,
scan `open[recorded..]` then `recorded = open.len()`, and `recorded = recorded.min(open.len())`
after each `EMC` pop. O(pushes+pops+removals). Measured: 60k x 60k **0.23 s**, 16k x 16k 0.59 s,
full suite still green. There is **no deadline checkpoint inside the walk** — checkpoints are
between calls in `redact_steps.rs` — and the walk runs **three times per stream**
(`page_carries` in `affected_streams`, `dropped` in `rewrite`, and again inside
`remove_glyphs_and_carried_text`).

## Odd-length property lists: no leak, but the rewriter emits malformed output

`as_chunks::<2>()` drops the trailing item in both `holds_text_key` and
`rebuilt_without_carried`. Odd `items` **is** reachable, and every odd shape either refuses
(`names_a_text_key` iterates *all* items, so a stray carried name is still seen) or rewrites
safely. Two measured manglings, canary absent in both:

- `<< /MCID 0 /ActualText 4 0 R >>` -> emitted `<< /MCID 0 0 R >>` — a dictionary keyed by a number.
- `<< /MCID 0 /Pad << /ActualText (X) >> /Tail >>` -> `<< /MCID 0 /Pad << >> >>`, `/Tail` silently gone.

## Round 2 mutation sweep: 12 planted, 11 applied and compiled, 1 discarded, **3 survived**

- **M6** widen `holds_text_key` back — survived; not a leak, see above.
- **M8** dedup set made inert — survived. Consequence measured: duplicate edits for the same
  span, and `Contents::apply` fails closed with `Malformed("pdf syntax: a content-stream edit
  that overlaps another...")` — an **unnamed, file-blaming** refusal that `redaction_corpus.rs`
  panics on, exactly the class `MarkedContentSplitAcrossElements` was added for.
- **M12** the `still_carries` internal self-check — survived; unreachable under the narrowing.
- Killed: M1 (`still_names_one`), M3 (`names_a_text_key`), M4 (nested recursion in the rebuild),
  M5 (`MarkedContentSplitAcrossElements`), M7 (`/E`), M9 (`BMC` opens a span — **now covered**,
  it survived in round 1), M10 (named `/Properties`), M11 (page-stream widening — **now covered**,
  it survived in round 1).
- M2 (`Carried::OpaqueString if false`) **did not compile** (non-exhaustive match). Discarded
  rather than counted — the reason this sweep checks compilation.

## Checked and could not break (round 2, all end to end through `redact::page`)

Nested dict, array item, `#`-escaped key `/Actual#54ext`, escaped **and** named together, hex
string, UTF-16BE, a string containing `>> \) <<`, dict-valued and array-valued `/ActualText`,
`%` comment inside the dict, empty-name key `/`, `/E`, three odd-dict shapes, indirect-reference
value, an extra operand after the dict (-> `properties-unresolved`), `BX`/`EX`, unbalanced `q`/`Q`,
stray `EMC`, an inner `BDC`, `/Contents` straddle in the dict and inside the string (both ->
`marked-content-split-across-elements`), carrier and glyphs in different elements, two removals
under one carrier, a 30,000-key property list, nesting to 62. **Depth 63+ is refused by the
lexer's `MAX_NESTING = 64`, so no recursion in this module can overflow the stack.** Inline
images whose data contains `EMC` are refused before the walk. PDFium extracted only the keep
line from every emitted output; each output has exactly one `%%EOF`/`startxref`, so no
incremental history.

Two shapes my scanner flagged are **correct**: a stray `EMC` closing the carrier before the
secret is drawn leaves the `/ActualText` in the file, but it belongs to an empty span and
PDFium extracts nothing from it.

## Round 1 findings, for the history

`/E` was missing from `TEXT_CARRYING_KEYS` (fixed here); `carrying_spans_over_removals` was
19,776 B -> 18.3 s (fixed, then re-found above); the page-stream widening and `BMC` had no
test (both now killed by mutations); `redaction_corpus.rs`'s reflow skip was a **byte scan**
for `/ActualText` or `/Alt` that disabled the check on 11 of 56 fixtures — now gated exactly
on `report.dropped_carried_text == 0`, which closes it.

Related: [[m2_standard14_and_marked_content]], [[m2_nested_form_lookup]], [[m2_redact_verify]],
[[spike_0006_redaction_survival]], [[m2_glyph_geometry]].
