# Code reviewer memory

- [Review expectations](feedback_review_expectations.md) — named design decisions must be challenged with a verdict; verify claims by running them.
- [Fuzz seed prefix check](feedback_fuzz_seed_prefix_check.md) — hand-decode the seeder's fixed 0x05 prefix through any new fuzz target; an inert seed corpus reads as coverage.
- [Edge injections on the live origin](project_edge_injections_on_live_origin.md) — Cloudflare zone features rewrite HTML after upload; verify the live origin with browser headers.
- [Inert detector path](feedback_inert_detector_path.md) — force a wrapped detector to find nothing with its inputs present; a wrapper that then prints OK is the bug.
- [Effective permissions blind spot](feedback_effective_permissions_blind_spot.md) — mutate the WORKFLOW-level `permissions:` block; jobs without their own inherit it and the rule still prints OK.
- [CI diagnostic steps lie](feedback_ci_diagnostic_step_lies.md) — run a failure-describing step against a real log: wrong build path, narrow regex class, and `if: failure()` all name the wrong cause.
- [New fuzz target registration](feedback_new_fuzz_target_registration.md) — four sites, not three; fuzz-nightly.yml's matrix is the seeded run and the one that gets missed.
- [Gate keyed on a hand-edited id](feedback_gate_keyed_on_hand_edited_id.md) — break the identifier a gate resolves through; an unresolvable case that is only a note means the gate turned itself off.
- [Probe that re-implements its rule](feedback_probe_reimplements_rule.md) — a source-scan test whose probe is a copied closure stays green with the real filter inert.
- [Probe fixture vs real producer](feedback_probe_fixture_vs_real_producer.md) — diff probe fixtures against the imitated tool's byte-exact output; an abridged fixture leaves a branch freely mutable.
- [Memoised walks under-count](feedback_memoised_walk_undercounts.md) — a `descended` set makes a nested child of a twice-drawn parent count 1; probe two levels deep.
- [Scope set wider than the edit](feedback_scope_set_wider_than_the_edit.md) — a caller-supplied "pages this covers" set the implementation does not honour; read back the pages it never edited.
- [Mutate the wiring, not the policy](feedback_mutate_the_wiring_not_the_policy.md) — a check whose policy has a lying fake still lets the real call site discard its result.
- [New guard shadows an old refusal test](feedback_new_guard_shadows_old_refusal_test.md) — an earlier argument check makes a re-pointed test pass on the wrong rule; print the real error.
- [Fuzz workspace is a separate build](feedback_fuzz_workspace_separate_build.md) — deleting a module can break sibling fuzz targets; `cargo +nightly check` every bin, not just the new one.
- [Calibration list vs accepted set](feedback_calibration_list_vs_accepted_set.md) — a hand-written FONTS array measures itself; diff it against the names the code accepts, and check exclusion lists survive aliasing.
- [Rewriter reach vs detector reach](feedback_rewriter_reach_vs_detector_reach.md) — a refusal replaced by a rewrite: feed the detector's widest shape to the rewriter; nesting is where they diverge.
