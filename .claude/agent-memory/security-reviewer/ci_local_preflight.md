---
name: ci-local-preflight
description: Measured gaps in tools/ci-local.py's environment preflight (2026-09-15) — it is non-transitive and its version half is skip-on-error; plus the self-test artifact/trap gaps.
metadata:
  type: project
---

`tools/ci-local.py`'s environment preflight (branch `ci-local-preflight`, 2026-09-15) derives its
required-tool set from `programs_in(job["run"])` only. Measured with cargo off PATH: `--only
integration-suites | checkers | prune-is-reached | subsetting-gate | checker-self-tests` all
return **zero findings**, and `tools/check-no-network-deps.sh` then prints the literal `cargo tree
failed` the preflight's own docstring names as the incident it exists to prevent. The gate is
**not transitive** — a job whose `run` is a script name requires only that file.

Version half is **skip-on-error**: `_reported_version` returns `None` on timeout/odd output, and
`check_versions` `continue`s. Simulated a `cargo --version` TimeoutExpired (realistic: rustup
auto-installing a bumped pin) → `findings: []`, `0 pinned version(s) compared`. The `compared`
count is printed and never gated, although the expected value is knowable from `PINS ∩ required`.

**The node pin moved to `.nvmrc` (2026-09-15, branch `ci-pin-node`).** `ci.yml` uses
`node-version-file: .nvmrc`; `ci-local.py`'s PIN reads the same file and carries `consumer` /
`conflict` regexes tying the pin to the step that reads it. Measured in a scratch copy: the
`conflict` rule does fire when both `node-version:` and `node-version-file:` are present
(`1 of 2 compared` plus a finding), and it is **not** exercised by `tools/test-ci-local.sh` —
only the consumer-replacement case is.

**`node` is in `required` only because ci.yml literally spells `node ../../tools/report-size-budget.mjs`.**
Measured: rewrite that step as `pnpm run size-budget`, update `JOBS` accordingly (parity forces
this), and the preflight prints `1 of 1 pinned version(s) compared` and **never compares node
again** — `due` shrinks with the requirement, so the count gate cannot see it. `pnpm` implying
`node` is the transitivity this gate does not have.

**`tools/test-ci-local.sh`'s `.nvmrc` restore was measured clean** (current version, lines
287-317 + 484-569 extracted into a standalone harness): INT/TERM/HUP all restore `22`; a missing
`.nvmrc` leaves the backup zero-byte so the `-s` guard correctly skips; the second case cannot see
a stale backup. Only SIGKILL leaves `99`.

Other measured facts, useful next time:
- bash runs an `EXIT` trap on SIGINT and SIGTERM but **not on SIGHUP** (exit 129, trap skipped).
  So in-place mutation of tracked files (`rust-toolchain.toml`, planted `core/*/tests/*.rs`)
  survives a closed terminal.
- Root `.gitignore` covers checker copies only as `tools/.*-fixture.{py,sh}` (dot-prefixed).
  `tools/*.copy.sh`, `tools/ci-local.broken.py` and `core/burrow-ops/tests/zz_*.rs` match
  nothing there and nothing in `check-no-generated-files.sh`'s 16 patterns.

**Why:** this is developer tooling whose entire value is refusing; a false green is the only
security-relevant failure mode, and two of the three above are false greens.

**How to apply:** when reviewing changes to `tools/ci-local.py` or any `tools/test-*.sh`, check
transitivity of derived requirements, whether an error path degrades to "skip", and whether a
self-test mutates a *tracked* file. See [[m1_ci_check_vacuity]].
