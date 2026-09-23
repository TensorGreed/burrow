---
name: new-guard-shadows-old-refusal-test
description: A new argument-validation guard added upstream can make an existing refusal test pass on the wrong rule; print the actual error text of every re-pointed refusal test.
metadata:
  type: feedback
---

When a change adds an **earlier** guard (a new `burrow-ops` argument check) and re-points existing
engine refusal tests at the new entry point, any test whose fixture trips the new guard now passes
without reaching the rule it was written for. Loose assertions (`text.contains("page")`) hide it.

**Why:** measured on #134 (`bab54c3`).
`redaction_defences.rs::a_page_index_past_the_end_refuses_rather_than_reaching_for_the_page` calls
page 7 with `redacted = {0}`. The new `page must be one the operation covers` check fires first, so
the test's stated target — the page-count bound that `ObjectHandle::page`'s `// SAFETY:` comment
relies on — is never reached. Commenting that bound out left the entire suite green.

**How to apply:** for every test the diff re-points at a new entry point, replace its assertion with
one that cannot match (`assert!(text.contains("ZZZ"))`) and read the printed error. Then mutate the
rule the test *claims* to kill and confirm it goes red. Pay particular attention when the shadowed
rule is an `unsafe` precondition.

Related: [[feedback_inert_detector_path]], [[feedback_gate_keyed_on_hand_edited_id]].
