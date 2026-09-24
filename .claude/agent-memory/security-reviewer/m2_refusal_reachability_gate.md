---
name: m2-refusal-reachability-gate
description: #177 witness gate in geometry.rs (6bc04a9) — proves a raise from *some* production line reached by a witness-chosen function, not from a production entry; measured plants and the MIR method for cfg(test)-only changes.
metadata:
  type: project
---

Reviewed 2026-09-24 at 6bc04a9 (73f5d64 landed on the branch after the brief and was NOT reviewed).

What the gate binds: `refuse` records `(rule, Location::caller().line())` under cfg(test); `prove` passes if
the witness returned an error `caught` by the rule AND some record has that rule at a line < `mod tests`.
- It does not bind the recorded raise to the returned error: a witness that drives a production raise which
  is swallowed, then returns `Refusal::X.refuse(..)` built in the test, passes (plant measured green).
- It does not bind to a production *entry*: witnesses call `remove_glyphs`, `writing_mode_of`,
  `check_type_three_procedure`, `check_form_sharing`, `carried_text_edits` directly.
  `remove_glyphs` has no non-test caller in the repo; GlyphFromAnotherStream is proven only at its raise in
  `remove_glyphs`, and redact_steps pre-filters `mine` by form before the other site, so neither site is
  reachable from production. Several witnesses forge `Glyph` fields (operation span, bytes_per_code).
- Swallowing `writing_mode_of` in `writing_mode_for` (`.or(Ok(Horizontal))`) survived the WHOLE suite.
- cfg(test) block / `cfg!(test)` / pub helper in production half all pass; a private helper is caught by
  clippy dead_code only.
- Gate self-mutations (no track_caller, no line check, no caught check, Ok passes) all caught by the probe;
  removing the `clear()` survives with no consequence found.

Production cost: verified by `cargo rustc --release -- --emit=mir` at commit and parent, stripping `//`
comments and span line:col — MIR identical, `refuse(_1: Refusal, _2: &str)` has no implicit Location arg.
Reusable method for any "cfg(test)-only" claim.

Plant pitfalls here: a planted enum variant needs `///` or clippy fails on missing_docs (wrong reason);
inserting a fn above `pub fn glyphs_in` steals its doc comment. Anchor helpers at `/// A 2-D affine transform`.
Full burrow-engines suite needs `tools/check-redaction-corpus.sh` run first (generated fixtures + manifest.json).

Related: [[m2-nested-form-lookup]], [[m2-glyph-geometry]].
