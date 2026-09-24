# burrow

Free, open-source, privacy-first file tools (PDF, image, video) that run entirely on the
user's device. One shared Rust core; three frontends (web, Android, iOS).
Licensed `MIT OR Apache-2.0`. No commercial licensing, ever.

## Non-negotiables

These are not preferences. A change that violates one is wrong, however well it works.

1. **Privacy.** File content never leaves the device. No code path that touches user file
   content may make a network call. No telemetry on file content, ever.
2. **Permissive licenses only.** Allowed: MIT, BSD-2/3, Apache-2.0, ISC, Zlib, MPL-2.0,
   OFL (fonts), Unicode-3.0, CC0-1.0, Unlicense — plus, for bundled native engine
   components, FTL, IJG, libpng-2.0, LicenseRef-AGG-2.3, MIT-Modern-Variant and ICU —
   plus NCSA, for libFuzzer (ADR 0012).
   Forbidden: GPL, LGPL, AGPL,
   SSPL, non-commercial, and anything unclear. CI enforces this in two halves:
   `cargo-deny` for Rust crates, `tools/check-engine-licences.py` against
   `engines/licenses.toml` for the engines. Adding a licence needs an ADR, not a config
   edit. See `docs/adr/0008-widened-licence-allowlist.md`, `docs/adr/0010-harfbuzz-and-icu-in-pdfium.md`
   and `docs/adr/0012-ncsa-for-libfuzzer.md`.
3. **All input is hostile.** Every file is untrusted and possibly adversarial. Every
   parser entry point gets a fuzz target. No panics cross the FFI boundary; errors are
   typed. Every operation takes a `Limits` and applies every ceiling in it. Input size, page
   count and pixel count are checked exactly, before anything is allocated. Time is
   cooperative and memory is **detected, not bounded** — see *Limits* below.
4. **Correctness before features.** Most of all in redaction, where a bug leaks secrets.
   Redaction output is verified automatically after every run.
5. **Tests are part of the feature.** Every operation ships unit, property (proptest),
   golden-file, and fuzz tests. CI must be green to merge.

## Commands

Rust (the cargo workspace root is the repository root; run these from there):

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all                 # cargo fmt --all -- --check to verify
cargo deny check                # licenses, advisories, bans
cargo audit                     # NOT optional when replicating CI -- see below
cargo build -p burrow-wasm --target wasm32-unknown-unknown
cargo doc --workspace --no-deps   # RUSTDOCFLAGS="-D warnings" in CI
```

Native engines (needed for `--all-features`; see `docs/adr/0004-native-engines.md`):

```bash
engines/fetch.sh              # pinned + checksum-verified engine artifacts
engines/build-native.sh       # zlib, libjpeg-turbo, qpdf (+ an ASan/fuzzer variant)
engines/build-wasm.sh         # the same engines for wasm
python3 tools/check-engine-licences.py   # engines/licenses.toml vs ADR 0008

cargo test --workspace --all-features    # links the engines; without --all-features it does not
```

Web (`apps/web/`):

```bash
pnpm install
pnpm dev            # local dev server
pnpm build          # static output to dist/
pnpm check          # astro check + svelte-check
pnpm lint           # prettier --check
pnpm test           # vitest
```

Fuzzing (`fuzz/`, its own workspace, nightly toolchain):

```bash
cargo +nightly fuzz list
cargo +nightly fuzz run <target> -- -max_total_time=60
```

## Where things live

| Path | Contents |
|---|---|
| `core/burrow-types/` | Typed errors, `Limits`, shared value types. No I/O, no engines. |
| `core/burrow-engines/` | Engine trait seams and FFI wrappers (PDFium, qpdf, …). |
| `core/burrow-ops/` | Operations (merge, split, redact, …) built on engines. |
| `core/burrow-core/` | Public API surface. The only crate bindings depend on. |
| `bindings/burrow-ffi/` | uniffi glue for Swift + Kotlin. |
| `bindings/burrow-wasm/` | wasm-bindgen glue for the web. |
| `apps/web/` | Astro + Svelte islands. One indexable page per tool. |
| `apps/android/`, `apps/ios/` | Native Compose / SwiftUI apps (M3 / M4). No WebViews. |
| `corpus/` | `manifest.toml` is tracked; `corpus/files/` is gitignored. |
| `tests/conformance/` | Small committed fixtures plus `expectations.json`, the typed outcome each must produce. Shared by the native tests and (from PR 4) the web differential harness, so it belongs to neither crate. |
| `tools/` | Corpus runner, visual diff, headless scripts. |
| `fuzz/` | cargo-fuzz targets, one per parser entry point. |
| `docs/adr/` | Architecture decision records. Add one for any decision worth asking twice. |

## Conventions

**Errors.** Every fallible function returns `Result<T, E>` with a concrete `thiserror`
enum. Error enums are `#[non_exhaustive]`. Never `unwrap()`, `expect()`, `panic!()`,
`todo!()`, `unimplemented!()`, or indexing that can panic in library code — tests and
`build.rs` may. Convert engine error codes into typed variants at the engine boundary;
do not let raw codes escape into `burrow-ops`.

**Panics and FFI.** No panic may cross an FFI boundary. Wrap entry points in
`catch_unwind` where a panic is conceivably reachable, and map it to an
`Error::Internal` variant. `panic = "abort"` is not an acceptable substitute.

**Limits.** Every operation takes a `Limits` and applies every ceiling in it. A missing limit
is a denial-of-service bug, not a nicety.

**They are not equally strong, and the difference is documented rather than smoothed over.**
`max_input_bytes`, `max_pages` and `max_pixels` are exact. `max_duration_ms` is cooperative:
overshoot of up to one engine call is possible, or, inside redaction's own geometry walk, one
content-stream lex (about 255 ms natively at the operand ceiling; ADR 0029, #175). **`max_memory_bytes` bounds nothing on any
platform** — a structural pre-scan and a length-based estimate before the engine, a measured
check after it, so an overrun is *detected*, not prevented. Say "detect" where we detect and
"bound" only where something is actually bounded; `burrow_types::Limits`' rustdoc and
`docs/adr/0007`'s 2026-09-12 amendment carry the per-path detail. An overclaiming doc comment
here is a bug, not a wording preference: this one was load-bearing in two later ADRs' reasoning
before PR 4b measured it.

**`unsafe`.** Crates default to `#![forbid(unsafe_code)]`. Only `burrow-engines` and
`burrow-ffi` may relax it, to `#![deny(unsafe_op_in_unsafe_fn)]`. Every `unsafe` block
carries a `// SAFETY:` comment stating the invariant it relies on and why it holds.

**Naming.** Crates and directories `burrow-kebab-case`; Rust items `snake_case` /
`CamelCase`; features `kebab-case`. Operations are verbs (`merge`, `split`, `rotate`).
Avoid abbreviations except the established ones (`pdf`, `dpi`, `ffi`).

**Commits.** [Conventional Commits](https://www.conventionalcommits.org/):
`feat|fix|docs|test|refactor|perf|build|ci|chore(scope): subject`. Imperative,
lowercase, no trailing period. Scope is the crate or app (`core`, `web`, `ci`).
Breaking changes get a `!` and a `BREAKING CHANGE:` footer.

**Dependencies.** Adding one is a decision, not a detail. Follow the `add-dependency`
skill and get a `license-auditor` pass before merging.

**Reviews run before the first push, not after.** The reviewer goes over the change while it
is still local. **Which reviewer depends on what the change decides** (2026-09-24):

- **Code that decides what redaction removes or keeps** gets both `security-reviewer` and
  `code-reviewer`. That covers the geometry walk, the rewriter, the refusal rules, verification,
  and anything else whose bug leaks a secret.
- **Tooling and gate changes** get one reviewer, whichever fits the change better. That covers
  checkers, their self-tests, CI wiring, fixture generators and docs.

When a change is both, it gets both.

Why before the push: In M1 PR 4a-ii they ran after the branch was pushed
and found two things that had already reached a commit: a reachable bug that took a page
offline after three long operations, and a regression that silently disabled two CSP tests by
turning an assertion into a tautology. Both would have been caught before anyone else could
pull them. A review that happens after the push is a review of history.

**A reviewer gets its own `git worktree`, never the tree being edited.** A review that is worth
running plants mutations to see what survives — that is how the last four found a `_malloc(0)`
guard nothing tested, a use-after-free the harness could not see, a scan test three offender
shapes walked past, and a probe set built against a fixture the producer never writes. Planting
happens in the working tree unless the reviewer is given somewhere else to stand.

Two sessions running at once over one tree is the failure: they see each other's mutations, and
so does anything else running there. Measured — one review reported methods appearing and
vanishing in the file it was reading, which were the other reviewer's plants, and a sweep started
in the same window was measuring a tree that changed under it. Nothing was wrong in the reports;
the point is that nobody could have told from the reports if something had been.

So: tell the reviewer to work in a throwaway worktree at `HEAD` and remove it when done, and do
not edit the tree or start a sweep while one is running. The corollary is that the change must be
**committed** before the review, which is the right order anyway — a reviewer reading
uncommitted work cannot say what it is reviewing.

## Definition of done

A change is done when all of these hold:

- [ ] `cargo fmt --all -- --check`, `cargo clippy … -D warnings`, `cargo test --workspace` pass.
- [ ] `cargo deny check` passes; any new dependency has a `license-auditor` pass and a
      `THIRD_PARTY_NOTICES.md` entry.
- [ ] New parser entry points have a fuzz target that runs clean for 60s **against a
      seeded corpus** (`python3 tools/seed-fuzz-corpus.py`). Unseeded, that sentence was
      hollow for the whole of M1: libFuzzer does not invent a valid PDF, so the targets
      were measuring the parser's rejection paths and nothing else. Measured — a defect
      planted in `reorder` survived 577,209 unseeded executions and died on the first
      seeded one. See `fuzz/README.md`.
- [ ] New operations have unit, property, golden, and fuzz tests, and enforce `Limits`.
- [ ] **The operation verifies its own output** through `burrow_ops::verify` before returning
      it (ADR 0022), with an `Expected` variant whose rustdoc states what it leaves
      undetectable — and a test where a fake engine lies on the way back, which the mutation
      that deletes the check fails. `split` and `compress` inherit this on arrival; neither
      ships without it.
- [ ] No new `unwrap`/`expect`/`panic!` in library code; new `unsafe` has `// SAFETY:`.
- [ ] No new network call reachable from code that touches file content.
- [ ] Any new check ships with **per-rule probes**: every pattern or rule matches its own
      positive fixture and rejects a near-miss, verified on **every run**, so the reported
      count is a measurement rather than a claim. A rule that matches nothing passes
      everything; one that matches everything fails everything; neither is a check.
- [ ] Any new check **reports what it examined**, and gates on the expected count where that
      count is knowable — not merely on non-zero. See *Working agreements*: "4 of 15" reads
      as success.
- [ ] The probe gate itself has a test: break a rule in a **copy** of the checker and assert
      it refuses, **naming the reason**. Put the copy beside the original — a copy in a temp
      directory resolves its own paths wrongly and exits non-zero for the wrong reason, which
      an exit-code-only assertion reports as a pass.
- [ ] Any new mutation or negative test **asserts the mutation applied** before running the
      suite it is meant to exercise.
- [ ] Public API changes are reflected in bindings (uniffi + wasm) or explicitly deferred.
- [ ] Docs updated: rustdoc on public items, plus `docs/ROADMAP.md` or an ADR if scope
      or a decision changed.
- [ ] `tools/ci-local.py --changed` passes **before every push**, and **GitHub CI is green on
      the PR's exact head commit before merge**. That green is the merge gate. No full local
      sweep runs before merge; that rule was dropped on 2026-09-24, and *Working agreements*
      says why. `ci-local` is the replication of CI, and it refuses to run if CI has a gate
      nothing local covers. The fuzz jobs are not on the pre-push path.
- [ ] Commit messages follow Conventional Commits; CI is green.

## Working agreements

### When to proceed, and when to stop and ask

The default is **proceed**. Stopping for confirmation has its own cost, and it was being
paid too often on changes nobody would have answered differently.

**Proceed without asking**, in the current PR or the next one:

- Test and check hardening — including a follow-up issue you filed yourself that is purely
  about making a check non-inert.
- Corrections to documentation, comments or stale notes you find to be wrong.
- Refactors within the scope of the PR you are already writing, **including ones that go
  slightly beyond the literal ask when the narrower change would leave something broken or
  rotting**. Say so in the summary.
- Filing issues, setting milestones, and fixing your own defects.

**Stop and ask:**

- Anything that changes a decision recorded in an accepted ADR.
- The licence allowlist, engine pins, or the acquisition route.
- Security posture: CSP, sandboxing, integrity pinning, the worker boundary, or what
  reaches an error message or a log.
- Anything that invalidates a measurement we rely on — size budgets, the licence audit,
  the conformance corpus.
- Milestone scope, or the order of the roadmap.
- Any choice where the cheaper option is **irreversible** and the reversible option is
  materially more expensive.

**Summarise per batch, not per PR.** Group related PRs and give one summary at the end,
naming each merge. Do **not** compress away mistakes, corrections or pushback: surfacing
those at full candour is working and is not what "summarise" is asking you to shorten.

### General

- Ask rather than assume. If two readings of a task lead to materially different work,
  raise it with a recommendation and the tradeoffs.
- Prefer `rg` over `grep`, and the file tools over shell text editing.
- Do not add docs, changelogs, coverage passes, or formatting sweeps that were not asked for.
- Do not commit or push unless asked.
- **A force-push names its branch and uses `--force-with-lease`.** `.claude/settings.json`
  denies the bare forms (`git push --force` with no refspec pushes the *current* branch, which
  may be `main`) and every spelling that targets `main`, including the `+main` refspec. Any
  other branch is allowed without a prompt, because rebasing a PR branch onto a merged base is
  routine — a squash-merge gives the base a new SHA, so a stacked PR that is only *retargeted*
  arrives conflicting and carrying its parent's commits again. `--force-with-lease` is the part
  that makes it safe: it refuses if the remote moved under you.

  Residual, stated rather than implied: a force-push to a branch someone else is working on is
  now permitted without a prompt. The lease catches the case where they have pushed; it does
  not catch the case where they have not pushed yet.
- **A CI monitor watches one run ID, and is stopped on every push.** Use
  `gh run watch <id> --exit-status`, never `gh pr checks <pr>`: the latter reports whichever
  run is *current*, so a monitor started for one commit silently begins reporting on another.
  Four of them accumulated across one PR, all polling the same endpoint — a green from any
  would have read as confirmation while saying nothing about the commit it was started for.
  Stop the previous monitor before starting the next.
- **An exit code is not an outcome. Read the state back.** This has now been measured three
  times, in three different tools, and each one reported success for something that had not
  happened:

  | | exit code | what was actually true |
  |---|---|---|
  | `gh run watch --exit-status` (M1 PR 4c) | **0** | the run's conclusion was `failure` |
  | `gh pr edit --base main` (M1, #76) | **0**, with a warning | the base was unchanged; it had failed on an unrelated GraphQL projects-deprecation error |
  | `grep -c` in an `&&` chain | **1** on a count of zero | the chain stopped; a successful restore read as a failed one |

  A fourth, adjacent: piping a runner through `tail` makes the *pipeline's* status `tail`'s,
  so `tools/ci-local.py … | tail -80` exited **0** over three failing jobs. Capture the status
  of the command you care about, not of whatever ran last.

  So:

  - **After any `gh` mutation — `pr edit`, `pr merge`, `pr create`, `api -X PATCH`, `run
    rerun` — read the state back and assert it.** `gh pr view <n> --json baseRefName,state`,
    `gh run view <id> --json conclusion`. `gh` prints deprecation and partial-failure
    warnings on stdout and still exits 0.
  - **`gh run watch --exit-status` is not the verdict.** Confirm with
    `gh run view <id> --json conclusion`, and read `--json jobs` when you need to know *which*
    job failed and whether it is yours.
  - Never put a command whose status you need on the left of a pipe.
- **Replicate CI with `tools/ci-local.py`, not by hand.** It reads `.github/workflows/ci.yml`,
  extracts every gate CI actually invokes — including the ones that run as *actions* rather
  than shell, like `cargo deny` and `cargo audit` — and **refuses to run anything** unless each
  one is either covered locally or carries an argued exemption that it prints. Add a check to
  CI and forget to add it there, and it fails before running a single test.

  ```bash
  tools/ci-local.py --changed  # BEFORE EVERY PUSH: the jobs the change can affect
  tools/ci-local.py            # every job, when you want it; no longer a merge gate
  tools/ci-local.py --check    # parity only
  tools/ci-local.py --list     # the coverage table
  tools/ci-local.py --only web # one job
  ```

  **The pre-push gate is `--changed`. The merge gate is GitHub CI, green on the PR's exact head
  commit.** Read the verdict per job with `gh run view <id> --json conclusion,jobs,headSha`, and
  check that `headSha` is the PR's head. That is two changes from "run everything before every
  push".

  **The first, in M2: the pre-push gate became `--changed`.** The old rule had stopped being
  followed. A full sweep is over twenty minutes, most of it fuzzing, and a rule that expensive
  gets skipped, half-run, or run against a tree that moved underneath it. All three happened.
  **A cheaper rule honestly applied beats an expensive one applied sometimes.**

  **The second, on 2026-09-24: the full local sweep before merge was dropped.** It repeated
  CI's own run on the same commit, and CI's run is the one that decides. What `--changed` cannot
  catch, CI catches before the merge, not after, provided the green is read from a run on the
  head being merged. A green from an earlier commit is not a green for this one.

  **Don't wait on CI.** Once a PR is pushed, start the next independent issue on a new branch,
  and come back when the run concludes. The CI-monitor rule above still holds: one watcher per
  run ID, stopped on the next push. "Independent" means the next branch does not build on this
  PR's unmerged changes. When it would, it waits, or it is stacked deliberately and said to be.

  `--changed` **narrows and never guesses**. It derives each job's paths from the command CI
  runs — the crate graph comes from the manifests, not from a map somebody maintains — and where
  it cannot attribute a changed file to a job, or cannot read the change from git, it runs
  everything and prints why. It reports what it ran and what it skipped, each with a reason,
  because a selective sweep that printed only its passes would read exactly like a full one.

  A hand-written path→job map was the obvious implementation and is the wrong one: it is the
  same shape as the coverage table this tool exists to replace, and it rots in the same way —
  the person adding a job is exactly the person who will not think to update it.

  **The fuzz jobs are off the pre-push path entirely.** A fuzz target is a *search*, and sixty
  seconds of it proves nothing about a change that did not touch the parser it fuzzes, while
  costing more than every other job combined. Searching belongs in `fuzz-nightly.yml`, which is
  seeded and given hours. The one exception is derived rather than declared: an edit **under
  `fuzz/`** puts them back, because `fuzz/` is its own cargo workspace and nothing else compiles
  a fuzz target — measured in M2, when a seam change broke `pdfsyntax_geometry` and every Rust
  gate run by hand stayed green.

  It exists because the rule that used to sit here did not work. That rule said *"replicating
  CI locally means every job, and `cargo audit` is the one that gets skipped"*, written after
  M1 PR 4c was pushed on twelve green local checks and went red on the thirteenth. It did not
  prevent the next three:

  | | missed | why it was invisible |
  |---|---|---|
  | M1 PR 4c | `cargo audit` | the only Rust gate that consults the network |
  | #63 | `tools/check-wasm-exports.sh` | run on the previous PR, not on this one |
  | #63 | the fuzz target list | `reorder` was in `Cargo.toml` and in no run list |
  | #63 | `pnpm check` | `lint` and `test` were run; `check` was not |
  | #54 | the subsetting gate | **the runner said it was covered** — see below |

  **A habit that has failed four times is not a control** — the same conclusion this file
  already reached about `git add -A`, and the same answer. Writing the rule down more firmly
  was not going to work a fifth time.

  The parity check runs in CI too, so the table cannot rot: the person adding a gate is exactly
  the person who will not think to update the local runner. `tools/test-ci-local.sh` re-plants
  every miss above and requires a refusal for each.

  **It refuses what it can SEE, and the fifth row is what that qualification cost.** #54's gate
  was a `run:` block invoking `cargo test -p burrow-ops --test split_no_leak -- --exact <name>`.
  The extractor mapped it to `cargo:test`, which the local `test` job already covered, so parity
  reported full coverage over a gate with no local counterpart — and the branch ran this tool
  clean and went red on that step. The sentence above was true of every miss before it and not of
  that one.

  Two changes, because one of them is a patch and the other is the rule. `--test <suite>` is now
  its own token, so a step naming one suite is a gate about that suite rather than a re-run of the
  workspace. And **a gate belongs in a script**: `tools/check-*.sh` is a name both CI and this
  runner can invoke, which is what gives the parity table something to track. A gate written
  inline is a gate betting that the extractor happens to have a pattern for its shape.
- **Never `git add -A` after running or building anything. Stage explicitly, or read
  `git status` first.** Twice this has put generated output on `main`:

  | | what landed | why the rule did not stop it |
  |---|---|---|
  | PR #29 | `tools/__pycache__/…​.pyc` | The rule did not exist yet. Verifying a tool by `importlib`-ing it had written the bytecode. |
  | PR #37 | 24 Playwright sweep logs under `spikes/**/results/` | The rule existed, in a **branch-local** `.gitignore` inside the spike directory. |
  | M2 #145 | `core/burrow-ops/tests/compress.proptest-regressions` | The rule existed and was read the same day. `cargo test --workspace` had just run **without the `qpdf` CLI on PATH**, so proptest wrote a seed for a failure that was environmental; `git add -A` staged it and it reached a commit. Caught reading the commit back, removed before any push. |

  The second is the instructive one. **An ignore rule that governs generated output belongs in
  the root `.gitignore`, never in a branch-local one** — checking out another branch deletes
  the tracked rule from the working tree, so it is absent at exactly the moment it matters.
  And a `.gitignore` cannot untrack what is already staged; `git rm -r --cached` is then the
  only way out.

  The third one is the one to sit with: the rule was not forgotten, it was **read and then not
  followed**, by someone who had spent that afternoon writing a checker for exactly that artifact
  (#148). Knowing the rule is not the same as the rule being enforced, which is the whole argument
  of this section — and `check-no-generated-files.sh` does not cover this case, because it scans
  *tracked* files and the seed was newly added in the same commit. #149 is the class-level fix.

  **A habit that has failed twice is not a control**, so `tools/check-no-generated-files.sh`
  now fails CI on any tracked file matching a generated-output pattern — `*.pyc`,
  `__pycache__/`, `results/`, `test-results/`, build output, `*.wasm`, the vendor tree. It
  scans the whole tracked tree rather than a diff, so it needs no merge base and stays red
  until the file is actually removed. Its self-test has one case per pattern, including both
  incidents by name.
- **Where a check lives in `ci.yml` is load-bearing, and consolidating jobs breaks checks
  silently.** `engines/vendor/` is gitignored and only some jobs fetch it, so a check that
  reads the vendor tree must live in a job that has one, and a check that reads only
  committed files should live in a job that fetches nothing so it still runs on a clean
  checkout. Two measured failures of this kind: the engine-licence *drift* comparison ran
  only in `deny`, which fetches nothing, so it was firing on **zero** components; and the
  qpdf crypto assertion lived inside a build step gated on a cache miss, so on an ordinary
  PR with a warm cache it did not run at all. **A check that silently examines nothing is
  worse than no check** — it reads as coverage. Before moving a step between jobs, ask what
  it reads and whether that will be there.

  That rule is necessary and it is **not sufficient**: it was written from those two
  failures and did not prevent a third two commits later. What catches this class is the
  next bullet — make the check say what it examined, so the log answers the question
  instead of the reader inferring it.
- **Every check reports what it examined, and gates on the expected count where that count
  is knowable.** A non-zero gate is not enough, because the failure mode is almost never
  zero. Measured instances, all of which printed `OK`:

  | check | examined | expected | |
  |---|--:|--:|---|
  | engine-licence drift, in CI | **4** | 15 | a real defect, fixed |
  | the same, with the resolver broken behind a zero-gate | **4** | 15 | why a zero-gate is not a gate |
  | `detect-engine-components.py`, CI vs a dev machine | **1** | 3 | probably fine — and unknowable from the output |

  The third row is the point as much as the first two. One artifact in CI may be entirely
  correct, because that job stages only the native tree — but the output is `OK` either way,
  so nobody can tell a legitimate 1 from a broken 1 without going and looking. A number
  with no expectation beside it is not a report.

  "4 of 15" reads exactly like success. So: print the count, and compare it against what the
  count *should* be whenever that is derivable — `check-python-syntax.py` compares its glob
  against `git ls-files '*.py'`, and `check-engine-licences.py` names every component whose
  original it could not resolve. Where the expected count genuinely is not knowable, **say
  what was examined by name** rather than printing a bare total.
- **A test harness that generates its own inputs is measuring what it can generate.** A fuzz
  target seeded from random bytes explores the shape of its parser's front door and nothing
  past it; a fixture whose pages are interchangeable cannot fail an order test; a marked
  document with a flat page tree cannot detect a flattening. All three happened here, and the
  first cost the most: every "fuzz target runs clean for 60 s" recorded during M1 was measured
  against a corpus grown from `/dev/urandom`, and seeding the targets from the committed
  fixtures found three real defects in minutes — one of them a ship blocker, two of them
  memory-unsafety in a pinned dependency. **Ask what the harness can produce before believing
  what it reports.**
- **A mutation test must assert the mutation applied before running the suite.** Twice in
  one session a `str.replace` silently matched nothing, the suite stayed green, and the
  green read as "this defence works" when nothing had been mutated at all. A mutation that
  does not apply is indistinguishable from a defence that holds. `assert old in s` before
  writing, and check the file actually changed — the assertion costs one line and is the
  only thing separating a real mutation sweep from a ritual.
- **A background shell is stopped when the work that started it ends, and the count of live
  shells is reported in every summary.** Standing requirement, asked for three times before it
  was written down.

  The failure is not theoretical. One session accumulated **seventeen** waiter shells, the
  oldest alive for six hours, every one of them the same bug:

  ```bash
  # WRONG. The waiting shell's own command line contains "playwright test", so `pgrep -f`
  # matches the waiter itself, the condition is never false, and the loop runs forever.
  until ! pgrep -f "playwright test" >/dev/null; do sleep 20; done
  ```

  `pgrep -f` matches against the full command line of every process **including the one that
  is doing the waiting**. A self-matching poll loop cannot terminate. It was diagnosed once,
  and then written a further fifteen times in the same session, which is why it is a rule here
  rather than a note in someone's memory.

  So:

  - **Prefer no waiter at all.** A backgrounded command notifies on completion by itself;
    polling for it is redundant as well as risky.
  - If a poll loop is genuinely needed, match on something that cannot describe the waiter —
    a marker line in the output file (`until grep -q DONE out; do sleep 5; done`), a pid, or a
    lock file. Never `pgrep -f` a string that appears in the loop itself.
  - **An `until` loop carries its own timeout.** The rule above says to match on a marker line,
    and that is not sufficient: a marker only arrives if the writer writes it. Two shells sat
    for **103 minutes** on `until grep -q '<marker>' log` and `until [ $(wc -l < log) -ge 14 ]`,
    over a log whose producer had exited at six lines with different labels. A self-matching
    loop cannot terminate because its condition is always true; these could not because it was
    always false, and the second kind is the one the marker-line advice produces. Bound it —
    `timeout 600 bash -c 'until …'`, or a deadline inside the loop — so a condition that never
    arrives ends the waiter rather than the session.
  - **Every summary states how many background shells are live, and the number comes from
    `ps`.** Not from what this turn launched: those are different numbers, and reporting the
    second while calling it the first is how a non-zero answer stays invisible. Through the
    103 minutes above the reported figure was "0" or "1" depending on what that turn had
    started — each time a true statement about the wrong set — and the two real shells surfaced
    only when someone asked directly.
  - **A subagent's children are yours to account for.** Those two belonged to a reviewer that
    had already handed back its report; the agent was recorded as finished and its shells were
    still running, so nothing in the task list showed them. A handback is not a reaping. Check
    `ps` after one, not just after your own work.
  - Zero is the expected answer, and saying "zero" is what makes a non-zero answer visible.
    Enumerate them with their parent command, not just a count, when the answer is not zero.
- Report faithfully. If tests fail, say so and show the output. Never claim a step passed
  without running it.
- **Review before the first push**: both reviewers for code that decides what redaction removes
  or keeps, and one for tooling and gate changes. See *Conventions*; this is the
  working-agreement half of the same rule.
