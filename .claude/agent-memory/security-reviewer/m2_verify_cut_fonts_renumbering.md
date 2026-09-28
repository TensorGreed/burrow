---
name: m2-verify-cut-fonts-renumbering
description: Redaction read-back check #2 (no orphaned /ToUnicode or /Differences mapping) examined no cut font in 151 of 170 native corpus verifications, because cut_fonts carries INPUT object ids and the witness keys the re-parsed OUTPUT's renumbered ids; plus the bridge-plant review of 54595d0
metadata:
  type: project
---

Found 2026-09-28 while reviewing 54595d0 (#137's bridge-qpdf.js plants). Pre-existing; not in that diff.

- **The defect.** `redact/mod.rs` fills `Cleared::cut_fonts` from `FontOutcome::font` = `pack(font.object())` on the INPUT document. `redact/witness.rs::mapped_codes` and `drawn_codes` key by `pack(font.object())` on a FRESH PARSE OF THE EMITTED BYTES. qpdf's writer renumbers, so the ids rarely match, and `region_is_cleared` does `if !cut_fonts.contains(font) { continue; }`: fail-open.
- **Measured (native, burrow-ops `redaction_outcomes`, eprintln in region_is_cleared):** 212 verifications; 170 with a cut font (first reported as 179; re-measured at 170 twice, once in review); 151 checked zero cut fonts, 15 checked 1 of 2, 4 checked all.
- **Demonstrated:** native `set_array_item` made a no-op → `the_differences_names_for_removed_codes_do_not_reach_the_output` fails on `/Sacute survived`, NOT on the redaction's `expect` — the redaction returned Ok. Debug line: `mapped={327680:…} cut={262144}`. On the web, the same no-op in bridge-qpdf.js over 04-differences-encoding left `[1 /B /U /R /O /W /hyphen /S /E /C /T /zero /four]` and replied OK.
- Only `redaction_defences.rs`'s byte assertions catch narrowing failures today; the hooks test checks cut_fonts is non-empty, not that it matches output ids.

**#137 bridge plants, for reference:** `armRedaction` splices the fetched redact bundle (bridge-qpdf.js is concatenated in, one occurrence); near-misses report applied=false. Plant 1's finding regex `/native redacted it; the web refused, Internal/` also matches a worker that throws at load ("no worker") and a throw-on-call; plant 2 drops only whitespace in every stream observed (content and /ToUnicode), so its OK is correct, not a verify miss. Corpus: plant 1 = 20 cases / 8 docs (18 Internal, 2 EngineUnavailable after the breaker trips), plant 2 = 113 / 56.

Related: [[m2-redact-verify]], [[m2-redaction-binding-137]], [[absence-needs-a-witness]].
