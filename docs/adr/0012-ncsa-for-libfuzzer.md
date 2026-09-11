# 0012. NCSA joins the allowlist, for libFuzzer

Date: 2026-09-10

## Status

Accepted. Amends [0003](0003-permissive-licensing.md) and
[0008](0008-widened-licence-allowlist.md), which is the required form: `deny.toml`'s
`allow` list is policy, and
[CLAUDE.md](../../CLAUDE.md) says changing it needs an ADR, not a config edit.

## Context

M1 PR 2 adds the first real fuzz target, which needs
[`libfuzzer-sys`](https://crates.io/crates/libfuzzer-sys) — the crate `cargo-fuzz`
exists to drive. It is the only way to reach libFuzzer from Rust.

Its crate metadata declares:

```toml
license = "(MIT OR Apache-2.0) AND NCSA"
```

The `AND` is the problem. `MIT OR Apache-2.0` covers the Rust wrapper and both are
allowed, but `AND NCSA` is conjunctive: NCSA is not an alternative we can decline. NCSA —
the University of Illinois/NCSA Open Source License — is not on our allowlist, so
`cargo deny check licenses` rejects the crate.

### What the source actually says, which is not what the metadata says

The crate vendors libFuzzer's C++ source under `libfuzzer/`. Every file in it carries:

```
SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
```

All 55 of them, with no exceptions and **no NCSA licence text shipped anywhere in the
crate** — only `LICENSE-APACHE` and `LICENSE-MIT`. That is the expected result of LLVM's
2019 relicensing from NCSA to Apache-2.0-with-LLVM-exception; the crate's metadata string
and its `README.md` were simply never updated to match.

`Apache-2.0 WITH LLVM-exception` is **already on our allowlist**.

So there are two defensible readings, and it matters which one we adopt:

1. **Go by the source headers.** The vendored code is Apache-2.0-with-LLVM-exception,
   already allowed, and the metadata is stale. Grant `libfuzzer-sys` an exception in
   `deny.toml` and move on.
2. **Go by the declared metadata.** The crate says NCSA, and the declared licence is what
   every machine check reads and what a downstream consumer would rely on. Admit NCSA.

## Decision

**Admit `NCSA` to the allowlist**, adopting reading 2.

`NCSA` is added to `deny.toml`'s `allow` list and to CLAUDE.md's non-negotiable #2.

Reading 1 was rejected even though its factual claim is correct, for two reasons:

- **An exception hides the decision in a place nobody re-reads.** `deny.toml`'s
  `exceptions` list is empty by design, and its comment says we would rather replace a
  dependency than grant one. Putting the first entry there — for a licence we have decided
  is acceptable, not for a crate we are tolerating — would misuse the mechanism and would
  make the *next* exception easier to add for worse reasons.
- **The source-header argument is ours, not upstream's.** We would be overriding a
  crate's own declared licence on the strength of our reading of its files. That is a
  judgement we might get wrong on some future crate, and it is not a habit worth
  starting. If the declared licence is one we can live with, the honest move is to say so.

And NCSA *is* one we can live with. It is a permissive BSD/MIT-family licence: attribution
and a disclaimer, no copyleft, no field-of-use restriction, no source-disclosure
obligation. It satisfies every criterion [ADR 0003](0003-permissive-licensing.md) actually
cares about. Its absence from the original list was because nothing needed it, not because
it was judged and rejected — the same reason ADR 0008 gave for the licences it added.

### Scope, stated precisely

`libfuzzer-sys` is a dependency of the **`fuzz/` workspace only**. It is:

- not a dependency of any crate under `core/` or `bindings/`;
- not linked into any shipped artifact — not the wasm module, not the mobile libraries;
- not in the root workspace's `Cargo.lock` at all (`fuzz/` is `exclude`d, with its own
  lock file).

So nothing we distribute carries NCSA code today. That narrows the consequence but is
**not** the justification: the licence is admitted because it is permissive, not because
it is confined to test tooling. If a shipped crate turned out to be NCSA tomorrow, this
ADR would already have allowed it, and that is the intended reading.

### The second finding: `fuzz/` was not being checked at all

Auditing this uncovered something worse than the licence itself. `fuzz/` is excluded from
the root workspace and has its own `Cargo.lock`, and CI's `deny` job runs `cargo-deny` at
the repository root with no `--manifest-path`. **No dependency of the fuzz workspace has
ever been licence-checked.** `libfuzzer-sys` would have entered unnoticed had this audit
not been run by hand.

CI now runs `cargo deny --manifest-path fuzz/Cargo.toml check licenses advisories sources`
against the same `deny.toml`. `bans` is deliberately excluded from that invocation:
`wildcards = "deny"` fires on the three `burrow-*` path dependencies, which is correct
behaviour for a crate we might publish and meaningless for a fuzz harness we never will.

## Consequences

One more licence on the list, and the list is the project's entire second non-negotiable,
so it should grow reluctantly. This is the second widening after ADR 0008; both were
driven by a dependency we could not avoid, and both were recorded rather than waved
through. A third should prompt the question of whether the list is describing the policy
or chasing it.

`fuzz/`'s dependencies are now in scope for licence and advisory checks, which is a real
improvement — it was an unaudited corner of the supply chain.

### Known gap this ADR does **not** close

**`cargo-deny` does not see dev-dependencies.** Measured with cargo-deny 0.20.2 on this
repository: `proptest`, `serde`, `serde_json` and their transitives do not appear in the
graph, and setting `[graph] exclude-dev = false` and clearing `targets` does not change
that. So "`cargo deny check` passes" currently means "the non-dev graph passes".

That matters beyond licensing: `deny.toml`'s `[bans]` deny-list — which names `reqwest`,
`hyper` and friends and is part of how non-negotiable #1 is enforced — does not apply to
any dev-dependency either. A test-only HTTP client would not be caught.

This PR's dev-dependencies were audited by hand instead, and are clean. The gap is
recorded in `deny.toml` and needs its own change; it predates this PR, and fixing it
properly means either upgrading cargo-deny, or generating the dev graph another way, which
is more than a licence decision.

## Alternatives considered

**Drop the fuzz target.** Would avoid the dependency entirely. Rejected outright:
non-negotiable #3 requires a fuzz target per parser entry point, and the roadmap names
this one. Dropping it to avoid a permissive licence would be trading a real safety
property for a bookkeeping one.

**A different fuzzing backend** — `afl.rs`, or `honggfuzz`. Both exist and both have
permissive licences. Rejected: libFuzzer is what OSS-Fuzz runs, it is what
`-max_total_time` / `-timeout` / `-rss_limit_mb` belong to (and ADR 0007 requires
`-timeout` specifically), and switching the whole fuzzing strategy to avoid adding a
BSD-family licence to a list is the tail wagging the dog.

**Grant `libfuzzer-sys` a `deny.toml` exception.** Reading 1 above. Rejected for the two
reasons given — the mechanism is for tolerating a crate, not for deciding a licence is
acceptable, and overriding a crate's declared licence with our own reading of its headers
is not a precedent worth setting.

**Ask upstream to fix the metadata.** Worth doing, and it would make this ADR moot for
`libfuzzer-sys` specifically. Not a plan we can block on: the fix would take a release
cycle, and we would still have to decide what to do in the meantime.
