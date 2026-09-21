---
name: new-fuzz-target-registration
description: Review technique — a new fuzz target has THREE registration sites plus a seeding shape; run ci.yml's own guard by hand rather than reading the diff
metadata:
  type: feedback
---

A burrow fuzz target is registered in four places, and a diff that touches three reads complete.
Run the checks instead of reading them.

- `fuzz/Cargo.toml`, `.github/workflows/ci.yml`'s `for target in …` loop, `tools/ci-local.py`'s
  `fuzz` job — and **`.github/workflows/fuzz-nightly.yml`'s `target:` matrix**, which is the one
  that gets missed (its own comment records four targets missing it before).
- The nightly matrix is where the **seeded** run lives while #62 is open — `ci.yml` runs targets
  unseeded on PRs — so a target absent there never runs seeded in CI at all, and the
  definition-of-done "clean for 60 s against a seeded corpus" is unmet.
- ci.yml's "Every fuzz target is actually run by this job" step compares `cargo +nightly fuzz
  list` against both workflows. Paste its three shell lines into a terminal; it takes seconds and
  gives a list.
- `tools/ci-local.py` does **not** cover that guard (no token for it in `--list`), so a green
  local parity run says nothing about it.
- Seeding shape: a target that consumes a leading byte as a parameter but is listed in
  `CARVED_TARGETS` rather than `PREFIXES` is never checked by `verify_prefixes`. Measure what the
  seeds decode to — `pdfsyntax_contents` left 33 of 38 seeds with a single element, i.e. the axis
  the target exists for.

Related: [[fuzz-seed-prefix-check]], [[feedback-review-expectations]].
