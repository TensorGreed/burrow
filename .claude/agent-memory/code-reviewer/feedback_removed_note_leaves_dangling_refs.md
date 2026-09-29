---
name: removed-note-leaves-dangling-refs
description: A dated ADR note that says "removed by the change that closes #N" leaves "the note above" dangling in its replacement; also check whether a state-flow comparison has equivalent mutants.
metadata:
  type: feedback
---

#218 round 4 (2026-09-28, 8475536). The UNBACKED note in ADR 0029 §6 listed exit criteria and said
the closing change removes it. The closing change did remove it. Its replacement then said "The
note above asked for coverage gated in the golden run", and a code comment in redact/mod.rs said
"the earlier note". Neither note existed at HEAD. The diff showed only additions, so the reference
read as fine.

Same round: `[font-selected-in-another-scope]` compared the full `Tf` route against the showing
route. Graphics state flows only parent to child (draw_form passes a clone and restores after), so
the Tf route is always a prefix of the show route. Full equality, length equality and "Tf ran in
this stream" are therefore the same test. A length-only mutation survived as an equivalent mutant.
That is not a defect, but "the full-route comparison is held" pins only the last-step mutation.

**Why:** a note that deletes itself on success is a common pattern here. Its criteria then live
only in git, so a sentence citing them points at nothing.

**How to apply:** when a diff removes a dated note, rg the HEAD ADR and code for "note above",
"earlier note" and the note's criteria wording. When a rule compares two pieces of walk state,
work out which directions state can flow before calling a surviving mutant a gap. Related:
[[cited-test-name-must-exist]], [[funnelled-check-plant-inert]].
