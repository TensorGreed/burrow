---
name: fuzz-workspace-separate-build
description: `cargo test --workspace` never compiles fuzz/; when a diff deletes or renames anything public, check every fuzz bin builds, not only the new one.
metadata:
  type: feedback
---

`fuzz/` is its own cargo workspace, so a green `cargo test --workspace` and a green
`cargo clippy --workspace --all-targets` say nothing about it. `cargo fuzz run <target>` builds only
that one bin, so adding a new target and running it proves nothing about the siblings.

Run, from `fuzz/`: `cargo +nightly check --bins` (or per-bin) with
`LD_LIBRARY_PATH=<repo>/engines/vendor/native-$(uname -m)/lib`.

**Why:** measured on #134 (`bab54c3`). The commit deleted `burrow_engines::redact_probe` and added
`redact_verified_output`, which builds. `redact_shared_contents.rs:184` still called
`burrow_engines::redact_probe::redact_page` and does not compile — so `ci.yml`'s fuzz loop and
`fuzz-nightly.yml`'s matrix both go red, and `tools/ci-local.py --changed` (which puts the fuzz job
back whenever `fuzz/` is edited) was evidently not run.

**How to apply:** any diff that removes a `pub` item, or that touches `fuzz/` at all, gets a build
of the whole fuzz workspace before anything else is judged.

Related: [[feedback_new_fuzz_target_registration]], [[feedback_fuzz_seed_prefix_check]].
