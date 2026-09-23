---
name: mutate-the-wiring-not-the-policy
description: When a check's policy module is unit-tested through a fake, mutate the real engine wiring instead — discarding the result or emptying its scope set survives.
metadata:
  type: feedback
---

A verification split into a **policy module** (tested with a lying fake) and **engine wiring**
(the closure that actually calls it) is almost always tested only on the policy side. Plant the
mutation in the wiring:

- discard the call's result — `let _ = check(...); Ok(())` at the real call site;
- feed the check an empty scope set — e.g. the `cut_fonts` filter in the closure that supplies
  what the check is allowed to look at.

**Why:** measured on #134 (`bab54c3`). The commit claimed a 15-entry sweep ending at 0 survivors,
and the policy module (`redact_verify.rs`) genuinely had a lying-witness test for every branch.
Both wiring mutations above, in `core/burrow-engines/src/qpdf/mod.rs`'s `redact_page_inner`,
survived the whole `burrow-engines` + `burrow-ops` suite. The sweep had mutated the policy and
the fake-driven `run`/`emit_verified` path, never the qpdf closure that joins them.

**How to apply:** whenever the diff has a `Fake`/`Liar` implementing the check's trait, ask which
file constructs the *real* implementation and mutate there. The fake's tests cannot see it by
construction — they never run the wiring.

Related: [[feedback_probe_reimplements_rule]], [[feedback_inert_detector_path]].
