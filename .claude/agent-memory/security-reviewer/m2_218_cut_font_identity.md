---
name: m2-218-cut-font-identity
description: "#218 ADR note review (2026-09-28): check 2's 27 'examined' cut fonts are id coincidences, 13 measured as a different font; mapped_codes never sees form-local fonts."
metadata:
  type: project
---

# #218: redaction check 2 keys input ids against output ids

Reviewed the docs-only ADR 0029 §6 note (commit a8dade6), 2026-09-28.

- Reproduced the owner's numbers exactly (212 / 170 / 213 / 27 / 151-15-4 / 0 orphans) with
  eprintln in `region_is_cleared`, `mapped_codes` and `steps::cut_fonts` (BaseFont per id).
- **"Examined 27" is not "27 cut fonts checked".** A match is output-id == input-id. By
  /BaseFont, 13 of the 27 were a *different* font (input obj 6 = BRWSPK+BurrowSpikeBlock cut,
  output obj 6 = Helvetica). At most 14 were the cut font.
- `mapped_codes` enumerates only the page's `/Font`; a form-local cut font is never in `mapped`
  whatever the ids (4 verifications had more cut fonts than page fonts). An id-only fix leaves it.
- Unqualified claim sites: ADR 0029 ~l.1773 list item 2 and ~l.1792 "cut / not cut → caught:
  yes"; `redact_verify.rs` header bullet 2 and `Cleared::cut_fonts` "Caught."; burrow-ops
  `redact/mod.rs` header promise 2.

**How to apply:** the #218 fix review should demand identity by what survives the write, a
form-scope enumeration in `mapped_codes`, refusal on an unmatched cut font, and a gated coverage
count. Method: needs `tests/redaction/generated` regenerated in the worktree
(`make-redaction-fixtures.py` + `make-evasion-fixtures.py`) and `engines/vendor` symlinked.

Related: [[m2_redact_verify]], [[m2_qpdf_redact_steps]].
