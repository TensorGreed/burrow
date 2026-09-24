---
name: m2-actualtext-rewriter
description: Review of d37556c (#165, the /ActualText marked-content rewriter) — the nested-key detect/remove asymmetry, the quadratic found.contains (19 kB to 18.3 s), the untested page-stream widening masked by band(), and 3 of 20 mutations surviving.
metadata:
  type: project
---

Reviewed `d37556c` on `m2/165-actualtext-rewriter`, 2026-09-24, in `/tmp/sec165`. Everything
below was **run**. Baseline `cargo test -p burrow-engines --all-features` was green.

## The asymmetry that matters: detection recurses, removal does not

`carried_by` → `holds_text_key` matches `/ActualText` or `/Alt` **at any depth** (its own comment
says so). `without_carried_keys` drops only **top-level** keys. So a span burrow classifies
`Carried::Text` can be "handled" while the string stays in the file. Measured, both `Ok`, canary
verbatim in `qpdf --qdf` of the output:

- `/Span << /MCID 0 /Extra << /ActualText (SEC) >> >>` → output `/Span << /MCID 0 /Extra << /ActualText (SEC) >> >>`
- `/Span << /MCID 0 /K [ /ActualText (SEC) ] >>` → unchanged

`main` **refused** both (`MarkedContentCarriesText`). So this is refuse → emit-with-the-string.
Not extractable by PDFium (a nested entry is not spec-honoured), but the commit's own doctrine is
byte presence: *"the string is still in the file — `qpdf --qdf` returns it."*

**Clean, all verified end to end:** `#`-escaped `/Actual#54ext`, `%` comments inside the dict,
hex-string values, a value containing `>>`, a dict-valued `/ActualText`, `<<>>`, two sibling
spans, an indirect ref (`/K 9 0 R`) before the key, `/Contents` array with `BDC` and glyphs in
different elements, one BDC over two removed show operations (one edit, dedup by
`!found.contains`).

## `/E` (expansion text) is a third carrier and is in neither list

PDF 32000-1 §14.9.5 allows `/E` in a marked-content property list. `TEXT_CARRYING_KEYS` is
`[ActualText, Alt]`. Measured: `/Span << /E (canary) /MCID 0 >>` redacts `Ok` with `/E` intact.
Pre-existing on `main` (same two keys), so not a regression.

## The quadratic is new in this commit: 19,776 bytes → 18.3 s

`carrying_spans_over_removals` ends with `for covering in &open { … !found.contains(covering) }`
— a **linear** scan of a growing `Vec`. `main` returned at the first removal site, so this is new.
No deadline checkpoint in the loop. Release, `max_duration_ms = 100`:

| fixture | | |
|---|--:|--:|
| 100 k nested `/S << /MCID 0 >> BDC` (no carrier) | 2.5 MB | 0.17 s |
| 100 k nested `/S << /ActualText (x) >> BDC` | 3.3 MB | 3.09 s |
| the same at 250 k, Flate-compressed | **19,776 B** | **18.3 s** (183x the deadline) |

Attribution proved by fixing it: `seen: BTreeSet<Span>` + `seen.insert(covering.properties)` →
**0.59 s** on the same file, suite still green. `MAX_TOTAL_OPERANDS` caps N near 262 k.

## The headline widening is untested, and `band()` is why

Mutation: delete the `streams.push(StreamId::Page)` widening in `affected_streams`. Full suite
**green** — and with a region over the secret only (`30,88,370,132`), three committed fixtures
leak their canary into the output: `evade-actualtext-around-a-form`,
`-around-a-nested-form`, `-over-a-form-without-resources`.

`redaction_defences.rs::band()` is `left 40, top 40, w 500, h 120`, which on the 400x200 fixture
page covers **both** the secret (y=100) and the keep line (y=40). The keep line is in the **page**
stream, so `StreamId::Page` is already in `streams` from a cut glyph and the widening never has to
fire. Any carrier test whose band also cuts a page glyph cannot exercise it.

**How to apply:** when reviewing anything that widens `affected_streams`, check whether the test
band cuts a glyph in the stream being widened. Same class as [[m2_redact_verify]]'s `glyphs_on`.

## Mutation sweep: 20 planted, 20 applied and compiled, **3 survived**

- **`BMC` no longer opens a span** — survived, and it is a live leak: `/Span<</ActualText(S)>>BDC
  BMC EMC BT (S) Tj ET EMC` emits the canary verbatim under the mutation. The inner `EMC` pops the
  carrying span. Exactly the "stack, not a flag" case the comment warns about, with no test.
- **The page-stream widening** — above.
- **`carried_text_edits`' `Carried::Unknown` refusal arm** — unreachable: `check_marked_content`
  runs first over page and every `form_scope` entry with identical arguments. Defence in depth,
  no probe.

## Two checks weakened, both worth remembering

- `redaction_corpus.rs` now skips the **reflow** assertion for any input whose raw bytes contain
  `/ActualText` **or `/Alt`** anywhere. The ADR says "skipped by name"; it is a substring scan.
  11 of 56 fixtures lose it today, three of them (`evade-struct-without-structparents`, both
  `nearmiss-*`) because their `/ActualText` is in the **structure tree**, which #165 never edits.
  `/Alt` is also a prefix of `/Alternate`, the ICCBased colour-space key.
- A `/Contents` array whose `BDC` dictionary **straddles** an element boundary (legal — §7.8.2 puts
  divisions between tokens) is now refused `Error::Malformed("pdf syntax: a content-stream edit
  that would write across the boundary…")` — a file-blaming variant with **no `[rule]` name**,
  which `redaction_corpus.rs`'s `Err` arm would panic on. `main` refused it by a named rule.

## Clean

No `unsafe`, no network, no logging, no `as` cast, no library-code `unwrap`/`expect` in the diff.
No file content in any new error message. `Contents::apply` fails closed on every overlap and
cross-boundary replacement I could construct. `Report` gained nothing, so the new §7 alternative-
text disclosure is documentation-only — there is no per-document accessor a frontend could show.

Related: [[m2_standard14_and_marked_content]] (the refusal this replaces),
[[m2_nested_form_lookup]], [[m2_redact_verify]], [[spike_0006_redaction_survival]].
