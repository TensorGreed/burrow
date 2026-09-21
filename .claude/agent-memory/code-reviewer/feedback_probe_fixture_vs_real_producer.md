---
name: feedback-probe-fixture-vs-real-producer
description: Review technique — diff a checker's probe fixtures against the byte-exact output of the tool they imitate; an abridged fixture leaves the load-bearing rule branch unprobed and fully mutable
metadata:
  type: feedback
---

When a burrow checker's `probe_the_rule()` fixtures imitate another tool's output (proptest's
persistence header, a linker map, a generated manifest), **go and read what that tool actually
emits** — the crate source in `~/.cargo/registry/src/*/`, not the docstring — and diff it
against the fixture. A hand-typed approximation probes the branches it happens to reach and
leaves the rest free to be deleted.

**Why:** measured on `tools/check-proptest-regressions.py` (2026-09-21). Its probes used a
3-line proptest header; proptest 1.11 writes 6 lines, so the line immediately above a real
seed is `BOILERPLATE[4]`, which no probe touched. Deleting `BOILERPLATE[3]` and `[4]` left all
six probes green, the whole four-case self-test green, and the exact file proptest writes
**accepted**. Two siblings of the same shape in the same file: no probe used the bare
`cc <hash>` line proptest tells a CI user to paste (no `# shrinks to` suffix), and no probe
used a second seed appended under a first, so both the seed-recognition predicate and the
no-comment branch had inert mutations that survived every probe.

**How to apply:** for each rule branch, ask *which probe fixture is the only one that reaches
it*, and whether that fixture is the shape the real producer emits. Then delete or neuter the
branch and re-run the probes AND the self-test — an inert mutation that survives both is the
finding. Restore from a scratchpad copy; work in a `git worktree`. Related:
[[feedback-inert-detector-path]], [[feedback-fuzz-seed-prefix-check]].
