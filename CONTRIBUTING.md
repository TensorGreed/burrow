# Contributing to burrow

Thanks for your interest. burrow is pre-alpha (M0), so the most useful contributions
right now are review, tooling, and tests rather than features.

Please read the [Code of Conduct](CODE_OF_CONDUCT.md) first. For security issues, follow
[SECURITY.md](SECURITY.md) instead of opening an issue.

## Before you write code

Open an issue first for anything beyond a small fix. burrow has strong constraints — see
the non-negotiables in [CLAUDE.md](CLAUDE.md) — and it is kinder to discuss an approach
than to reject a finished pull request.

Three things get pull requests closed regardless of code quality:

1. **A network call reachable from code that touches file content.** File content never
   leaves the device. This is the whole point of the project.
2. **A dependency that is not permissively licensed.** See [ADR 0003](docs/adr/0003-permissive-licensing.md).
   CI will catch it, but please check first.
3. **An operation without tests.** Tests are part of the feature.

## Setting up

```bash
rustup show                     # rust-toolchain.toml pins the version for you
cargo build --workspace
cargo test --workspace
corepack enable pnpm && pnpm -C apps/web install
```

`cargo-deny`, `cargo-audit`, and `wasm-pack` are needed to run the full CI set locally:

```bash
cargo install --locked cargo-deny cargo-audit wasm-pack
```

## The loop

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

Every command above runs in CI. If one fails locally it will fail there too.

## Adding an operation

Follow the checklist in [`.claude/skills/add-operation/SKILL.md`](.claude/skills/add-operation/SKILL.md).
Summarised: core implementation → typed error variants → `Limits` enforcement → unit,
property, golden, and fuzz tests → bindings → web page → docs. All of it, in one pull
request, or split into reviewable pieces that each pass CI.

## Adding a dependency

Adding a dependency is a decision, not a detail. Follow
[`.claude/skills/add-dependency/SKILL.md`](.claude/skills/add-dependency/SKILL.md):
license check, maintenance health, binary-size delta, security history, and an entry in
`THIRD_PARTY_NOTICES.md`. Say in the pull request why the dependency is better than
writing the code yourself.

## Commits and pull requests

[Conventional Commits](https://www.conventionalcommits.org/):
`feat|fix|docs|test|refactor|perf|build|ci|chore(scope): subject` — imperative,
lowercase, no trailing period. Scope is the crate or app (`core`, `web`, `ci`).

Keep pull requests focused. A description that explains *why* is worth more than one that
restates the diff. Note explicitly if you changed anything touching privacy, licensing,
`unsafe`, or resource limits.

## Definition of done

The checklist in [CLAUDE.md](CLAUDE.md#definition-of-done) is the standard a change is
reviewed against. Please run through it yourself before asking for review.

## Licensing of contributions

Contributions are accepted under the same terms as the project: `MIT OR Apache-2.0`.
By opening a pull request you agree your work may be distributed under both. There is no
CLA.
