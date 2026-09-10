---
name: add-dependency
description: Checklist before adding any dependency to burrow — Rust crate, npm package, native C/C++ engine, or font. Covers license, maintenance health, binary size, security history, and a license-auditor pass. Use whenever a Cargo.toml, package.json, or vendored engine changes.
---

# Adding a dependency

A dependency is a permanent decision. In burrow it is also a licensing commitment, a
supply-chain risk on the code path that handles people's private files, and — for the
web — bytes every user downloads.

The default answer is **no**. Work through this before proposing one, and be prepared to
say why the dependency is better than writing the code yourself.

## 0. Do you need it?

- What exactly does it do that you would otherwise write? If the answer is a few hundred
  lines you understand, write them.
- Is it already in the graph transitively? `cargo tree -i <crate>` /
  `pnpm why <package>`. Reusing something already there is free; adding a second
  dependency that does the same job is not.
- Does the standard library, or a crate already present, cover it?
- Is this dependency in the core, where the privacy guarantee lives, or in a frontend?
  The bar is highest for `core/` and `bindings/`.

**Hard stop:** if it can make a network call, or pulls in an HTTP client, it does not go
anywhere in `core/` or `bindings/`. That is non-negotiable #1, not a preference.

## 1. Licence

Read `docs/adr/0003-permissive-licensing.md`. Check the licence *before* spending time on
anything else.

**Allowed:** MIT, BSD-2-Clause, BSD-3-Clause, Apache-2.0 (incl. WITH LLVM-exception),
ISC, Zlib, MPL-2.0, OFL-1.1 (fonts), Unicode-3.0, CC0-1.0, Unlicense. For **bundled
native engine components only**, ADR 0008 also allows `FTL`, `IJG`, `libpng-2.0` and
`LicenseRef-AGG-2.3`, and ADR 0010 adds `MIT-Modern-Variant` and `ICU`.

**Forbidden:** GPL, LGPL, AGPL, SSPL, any non-commercial or field-of-use restriction, and
**anything you cannot determine**. Unclear means forbidden.

Check these specifically:
- The real licence field in the manifest, and the actual LICENSE file — not the registry
  summary or a README badge.
- **Transitive** dependencies, not just the direct one. `cargo deny check licenses`
  covers Rust across every target we ship to; npm and native C/C++ you check by hand.
- `A AND B` licensing where B is forbidden. `A OR B` is fine — we may pick.
- For native engines: any `third_party/` or vendored directory *inside* the engine. A
  permissive headline licence can bundle forbidden code. PDFium bundles several licences;
  jbig2enc pulls in Leptonica.

Do not add an exception to `deny.toml`. Its exceptions list is empty by design, and
changing the allowlist requires a new ADR (0008, as amended by 0010).

A licence may pass ADR 0008's **admission test** (permissive; no copyleft; no
field-of-use or non-commercial restriction; obligations limited to notices and credit
lines) and still not be allowed. Passing the test only makes it *eligible* to be added by
an ADR. The test explains the policy; it does not bypass it.

If the dependency is a **native engine** rather than a crate or npm package, run
`tools/detect-engine-components.py` against the built artifact — reading its licence
directory is not enough, as HarfBuzz proved — and add an entry to `engines/licenses.toml` — component, version, SPDX id or `LicenseRef-`,
licence file, whether it is confirmed linked, and any notice obligation.
`tools/check-engine-licences.py` fails CI otherwise.

## 2. Maintenance health

You are taking on this code's future, including its security fixes.

- When was the last release? When was the last commit?
- How many maintainers actually merge things? A single-maintainer crate in the core is a
  real risk — note it explicitly.
- Are there open issues describing correctness or safety problems that nobody has
  answered?
- How does it handle breaking changes? Does it have a changelog you could actually use
  during an upgrade?
- Download and reverse-dependency counts, as a weak signal of how many other people would
  notice a problem.
- Is it a candidate for takeover? A popular, unmaintained package with a transferable
  name is a supply-chain hazard.

## 3. Binary size

- **Rust/wasm:** build before and after and record the delta.

  ```bash
  cargo build -p burrow-wasm --target wasm32-unknown-unknown --release
  ls -l target/wasm32-unknown-unknown/release/*.wasm
  ```

  The wasm module is already dominated by PDFium; every added kilobyte is downloaded by
  every user before their first operation.

- **npm:** check the installed size and whether it tree-shakes. A dependency that pulls a
  framework runtime onto a page that ships no JavaScript defeats
  [ADR 0005](../../../docs/adr/0005-web-stack.md).

- **Native engines:** measure the static-link size per target, not just on the host.

- Does it bring heavy transitive dependencies, proc-macros that slow every build, or a
  second copy of something already in the graph? `cargo tree --duplicates`.

## 4. Security history

- Check the advisory database: `cargo audit`, and search
  [RUSTSEC](https://rustsec.org/) / [GitHub advisories](https://github.com/advisories) /
  the CVE list by name.
- Past advisories are not disqualifying — how they were *handled* is the signal. Fast,
  clear, well-communicated fixes are a good sign.
- Does it use `unsafe`? How much, and is it justified and documented? A parser crate with
  undocumented `unsafe` is a worse bet than one that is a little slower.
- Does it do anything at build time? A `build.rs` or an npm `postinstall` that fetches
  from the network executes on every contributor's machine and in CI. pnpm blocks install
  scripts by default here (`pnpm-workspace.yaml` `allowBuilds`) — keep it that way.
- For a native engine: is it fuzzed upstream? Is there an OSS-Fuzz project?
- Is the version pinned with a checksum, and is provenance recorded? No build-time fetch
  from an unpinned URL, ever.

## 5. Pin it properly

- Rust: an explicit version in `[workspace.dependencies]`, referenced by member crates
  with `.workspace = true`. Commit `Cargo.lock`.
- npm: an exact-enough range, and commit `pnpm-lock.yaml`. CI runs
  `--frozen-lockfile`.
- Native engines: pinned version **and** checksum, with the source recorded.
- Prefer `default-features = false` and enable only what you need. Fewer features is less
  code, less size, and less attack surface.

## 6. Record it

Update `THIRD_PARTY_NOTICES.md` **in the same change**. This is a distribution
requirement of the MIT, BSD, and Apache-2.0 licences — the notices must travel with the
binary — not bookkeeping. Record the exact licence string, the version, and what uses it,
and mark it if it is compile-time only.

## 7. Get a license-auditor pass

Run the `license-auditor` agent. It must return **PASS** before merge. A **NEEDS
DETERMINATION** verdict is blocking, not advisory.

## 8. Verify

```bash
cargo deny check                 # licenses, advisories, bans, sources
cargo audit
cargo tree --workspace --duplicates
cargo build --workspace && cargo test --workspace
pnpm -C apps/web install --frozen-lockfile && pnpm -C apps/web build
```

## In the pull request

State plainly:

- What the dependency does, and why it beats writing the code.
- Its licence, and its transitive licences if any are unusual.
- The size delta you measured — the number, not "small".
- What you found on maintenance and security, including anything that gave you pause.
- Where it sits: `core/`, `bindings/`, or a frontend.

A dependency change reviewed with "CI is green" is not reviewed. `cargo-deny` proves the
licence is permitted; it says nothing about whether the dependency is a good idea.
