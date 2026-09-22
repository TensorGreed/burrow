---
name: m2-remove-glyphs-and-sharing
description: Measured facts from the PR #156 review (m2/131-remove-glyphs, 8820578) — the ' and " reflow, the ragged-code leak, the transitive sharing under-count, the Type 3 font exponential, and which qpdf fakes were verified against the real engine.
metadata:
  type: project
---

Reviewed branch `m2/131-remove-glyphs` at `8820578` (base `3fb1e3c`) on 2026-09-22, in a
throwaway worktree. Everything below was reproduced; the probes were planted and removed.

**Why:** the brief was "review the fakes against real qpdf", because every test on the branch
runs against fakes. Three of the four qpdf behaviours checked turned out to match; the defects
were in burrow's own arithmetic and in the *test fixtures*, not in the qpdf model.

## Real qpdf behaviour, verified from `engines/vendor/src/qpdf-12.4.1` and by running it

- `qpdf_oh_get_key` on a **non-dictionary** → `QPDF_Dictionary.cc:228` `typeWarning` + a null
  object. A **warning**, not an error. `qpdf_has_error` does not see it, so `take_error()`
  correctly returns `None` — the fake matches.
- `qpdf_oh_get_type_code` fallback under `trap_oh_errors` is `ot_uninitialized` (0),
  `qpdf-c.cc:1088`. It always accompanies a latched error, and `sharing.rs` checks
  `take_error()` after every `type_code()`, so the 0 arm is not reachable without the error.
- **Warnings are retained forever.** `Common::warn` (`QPDF.cc:342`) does
  `m->warnings.emplace_back(e)` regardless of `suppress_warnings`; only the *printing* is
  suppressed. burrow sets no `max_warnings`.
- `qpdf_oh_replace_stream_data` with a **null** filter oh: `Stream::replaceFilterData`
  (`QPDF_Stream.cc:770`) does `if (filter) replaceKey("/Filter", filter)` — a `newNull()` is
  *initialized*, so the key IS replaced with null. **Measured end to end** on a genuinely
  `/FlateDecode` fixture: `/Filter` type 7 → 2, the write re-compresses and emits a correct
  `/Filter /FlateDecode` + `/Length 16`, and the reopen reads the plain replacement back. No
  stale filter, no stale `/Length`. This one is clean.

## Defects measured

- **`'` and `"` lose the line move and the spacing operands.** `rewrite_without`
  (geometry.rs) turns every show operation into `[...] TJ`, so `'`'s implicit `T*` and `"`'s
  `aw ac` are dropped. PDFium, through the PR's own `origins_across_a_redaction` oracle:
  every kept glyph moves **14 pt** up; for `"` the horizontal spacing collapses as well
  (B/C/D at 129.4/139.6/149.8 → 117.4/124.6/131.8). No committed fixture uses either operator.
- **Ragged last code re-slices the string and leaves the removed byte in.** `emit_run` takes
  `width` from the **first cut glyph's** `code_len`, and `show` records `chunk.len()`, which is
  short for the trailing partial chunk. 2-byte font, `<414243> Tj`, remove code 1 →
  `[<41> -500 <43> ] TJ`: the removed code's byte survives and the kept 2-byte code is
  destroyed.
- **Transitive form sharing is under-counted.** `Walk::descend`'s `descended` memo means a
  nested form's subtree is counted once however many parents reach it. Two pages drawing form
  A, A's `/Resources` naming form B: A = 2, **B = 1**. `check_form_sharing` then permits the
  in-place edit that strips B's text from the other page — the exact direction the ADR 0029
  amendment says is unsafe, in a graph the amendment's own table lists.
- **A shared `/Contents` is not covered at all.** Nothing counts uses of a page's own content
  stream; two pages with `/Contents 5 0 R` is legal and is what `inheriting_document()` in
  `sharing_tests.rs` already builds.
- **Type 3 font `/Resources` recursion is not memoised → b^32.** Only `descend` (streams)
  memoises; `type_three_fonts` → `resources(font/Resources, depth+1)` has no memo. Branch-2
  ladder, measured release: depth 18 = 0.62 s, 20 = 2.5 s, **22 = 10.1 s from 6,021 bytes**.
  Extrapolates to ~2.9 h at the depth-32 cap, ~8.6 kB. A *cyclic* version refuses in 209 µs
  because the depth error propagates — the DoS needs a **terminating** ladder.
- **`/Annots` of non-dictionaries costs ~640 B of retained qpdf warning each.** 400,000
  integers, 800 kB file → **265 MB VmHWM**, 400 ms; the same count of empty dicts (2 MB file)
  costs 120 ms. `count_form_uses` takes no `Limits` and no `Deadline`.

## Vacuous or comment-satisfiable checks

- `write_path_tests::a_null_filter_leaves_the_stream_uncompressed_and_readable` asserts
  `/Filter` type == 2 **after** the replace. `pdf_with_ink()` has **no `/Filter`**, and
  `qpdf_oh_get_key` on an absent key also returns a null — measured: the assertion already
  holds *before* the call. The behaviour it claims to measure is real, the test does not
  measure it.
- `tools/test-name-requires-slash.sh` compiles a **copy** of the predicate and guards drift
  with `grep -q 'matches!(bytes, ...)'`. Deleting the `assert!` from `Name::literal` and
  leaving the predicate in a `//` comment: the script prints
  `OK -- 6 probe(s): 4 rejected spellings refused at compile time`, and all four `name::tests`
  pass — including `the_compile_time_check_is_real_and_not_a_comment`. Same class as the two
  comment-satisfiable probes in [[m2-glyph-geometry]].

## Checked and clean

Handle accounting returns to baseline on **every** refusal path (form cycle, resource depth,
type-3 ladder) as well as the committed 200-page success path. No new `unsafe`, no network, no
dependency change, no file content in any new error message (`ops::number` explicitly refuses
to interpolate). `Refusal::caught` is still a sound oracle: prefix + `Error` variant, not
`contains`. `region.rs`'s four rotation arms are each correct — I re-derived all four by hand
against the corner mappings.

See [[m2-glyph-geometry]] for the front-vs-back operand indexing and form-DAG findings this
branch builds on, and [[spike-0006-redaction-survival]] control 5.
