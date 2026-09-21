# Code reviewer memory

- [Review expectations](feedback_review_expectations.md) — named design decisions must be challenged with a verdict; verify claims by running them.
- [Fuzz seed prefix check](feedback_fuzz_seed_prefix_check.md) — hand-decode the seeder's fixed 0x05 prefix through any new fuzz target; an inert seed corpus reads as coverage.
- [Edge injections on the live origin](project_edge_injections_on_live_origin.md) — Cloudflare zone features rewrite HTML after upload; verify the live origin with browser headers.
- [Inert detector path](feedback_inert_detector_path.md) — force a wrapped detector to find nothing with its inputs present; a wrapper that then prints OK is the bug.
- [Effective permissions blind spot](feedback_effective_permissions_blind_spot.md) — mutate the WORKFLOW-level `permissions:` block; jobs without their own inherit it and the rule still prints OK.
- [CI diagnostic steps lie](feedback_ci_diagnostic_step_lies.md) — run a failure-describing step against a real log: wrong build path, narrow regex class, and `if: failure()` all name the wrong cause.
- [New fuzz target registration](feedback_new_fuzz_target_registration.md) — four sites, not three; fuzz-nightly.yml's matrix is the seeded run and the one that gets missed.
- [Gate keyed on a hand-edited id](feedback_gate_keyed_on_hand_edited_id.md) — break the identifier a gate resolves through; an unresolvable case that is only a note means the gate turned itself off.
