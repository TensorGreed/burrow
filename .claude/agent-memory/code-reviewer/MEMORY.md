# Code reviewer memory

- [Review expectations](feedback_review_expectations.md) — named design decisions must be challenged with a verdict; verify claims by running them.
- [Fuzz seed prefix check](feedback_fuzz_seed_prefix_check.md) — hand-decode the seeder's fixed 0x05 prefix through any new fuzz target; an inert seed corpus reads as coverage.
- [Edge injections on the live origin](project_edge_injections_on_live_origin.md) — Cloudflare zone features rewrite HTML after upload; verify the live origin with browser headers.
- [Inert detector path](feedback_inert_detector_path.md) — force a wrapped detector to find nothing with its inputs present; a wrapper that then prints OK is the bug.
- [Gate keyed on a hand-edited id](feedback_gate_keyed_on_hand_edited_id.md) — break the identifier a gate resolves through; an unresolvable case that is only a note means the gate turned itself off.
