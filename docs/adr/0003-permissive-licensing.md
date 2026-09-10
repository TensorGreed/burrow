# 0003. Permissive licenses only, enforced by CI

Date: 2026-09-10

## Status

Superseded by [0008](0008-widened-licence-allowlist.md).

The reasoning below still holds and is still the reason we reject copyleft. What changed
is the allowlist itself: it was drawn up for Rust crates and proved too narrow to
describe a bundled C/C++ PDF engine. ADR 0008 widens it, adds an admission test, and
records the notice obligations that come with it. This body is left as written.

## Context

burrow is free and open source, with no commercial licensing of any kind — no paid tier,
no dual-licensing upsell, no enterprise edition. That is a fixed property of the project,
not a launch strategy.

That intent creates a licensing constraint rather than removing one. burrow ships as
**statically linked native binaries** — an iOS app, an Android app, and a WebAssembly
module — built on vendored C/C++ engines. In that shape:

- A **GPL** dependency would require distributing the whole app under the GPL, which is
  incompatible with the App Store's terms and would force our own `MIT OR Apache-2.0`
  licensing to change.
- An **LGPL** dependency's relinking requirement is effectively impossible to satisfy in
  a statically linked wasm module or a signed iOS binary. The LGPL is only comfortable
  when dynamic linking is available, and here it is not.
- **AGPL** and **SSPL** are irrelevant to on-device software in theory, but a dependency
  under either signals a licensor who intends copyleft reach; and if burrow ever grew any
  server component, the AGPL would apply to it.
- A **non-commercial** clause would prevent our users — including businesses — from using
  burrow for their work, which defeats the purpose of shipping a free tool.

The engines we want mostly cooperate: PDFium is BSD-3-Clause, qpdf is Apache-2.0,
HarfBuzz is MIT, libwebp and mozjpeg are BSD, libavif is BSD-2-Clause. But the PDF and
image ecosystems are full of GPL alternatives (Poppler, Ghostscript, MuPDF under AGPL
without a commercial license), and those are frequently the first search result. This is
a mistake that is easy to make and expensive to unwind: by the time a GPL library is
woven into the core, removing it means rewriting the feature.

Licence obligations also apply to **fonts** and **test corpora**, not just code, and both
are easy to overlook.

## Decision

We will accept dependencies under these licenses only:

**MIT, BSD-2-Clause, BSD-3-Clause, Apache-2.0 (including WITH LLVM-exception), ISC,
Zlib, MPL-2.0, and OFL-1.1 for fonts.**

We will reject: **GPL (any version), LGPL (any version), AGPL, SSPL, any
non-commercial or field-of-use restriction, and anything whose license we cannot
determine.** Unclear licensing is treated as forbidden, not as a research task to defer.

MPL-2.0 is included deliberately: its copyleft is per-file and explicitly compatible with
distributing a larger work under other terms, so it is safe for us where the LGPL is not.

Our own code is `MIT OR Apache-2.0` — the Rust ecosystem default, giving users the
patent grant of Apache-2.0 and the simplicity of MIT.

**This is enforced by CI, not by review.** [`deny.toml`](../../deny.toml) encodes the
allowlist and `cargo deny check` runs on every commit, across every target we ship to
(including iOS, both Android ABIs, and wasm) so a platform-specific dependency cannot
slip in through a target CI's host does not build. The exceptions list is empty by
design, and `unknown-registry` and `unknown-git` sources are denied.

Additional requirements:

- Every dependency, including native C/C++ engines and fonts, is recorded in
  [`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md) in the same pull request that
  adds it. This is a distribution requirement of the MIT, BSD, and Apache-2.0 licenses,
  not bookkeeping.
- New dependencies go through the `add-dependency` skill and a `license-auditor` pass.
- Corpus files carry a `license` field in `corpus/manifest.toml` and a
  `redistributable` flag, so a non-redistributable test file can still be used locally
  without ever being published.
- **Changing the allowlist requires a new ADR superseding this one.** Editing
  `deny.toml`'s `allow` list is a policy change, not a configuration tweak.

`cargo-deny` covers Rust and its sources but not npm or vendored C/C++. Those are the
`license-auditor` agent's responsibility until tooling covers them.

## Consequences

Users and downstream distributors can use burrow for anything, including commercially,
without reading a license lawyer's opinion. The App Store and Play Store paths stay open.
Our own dual license cannot be forced to change by a dependency.

The cost is that some of the best tools in this domain are off-limits. Ghostscript and
MuPDF are excellent and both are AGPL-without-a-commercial-licence; Poppler is GPL. We
will occasionally have to implement something ourselves, accept a less capable engine, or
do without a feature — and "just use Ghostscript" will keep being suggested. That is the
trade we are making knowingly.

`cargo-deny` will also block legitimate work occasionally: a transitive dependency
changing license, or a crate with imprecise SPDX metadata. The response is to fix the
dependency graph or raise the confidence threshold's specific case, never to add a
blanket exception.

A single non-compliant dependency discovered late is expensive, so the enforcement runs
on every commit rather than at release time.

## Alternatives considered

**Allow LGPL with dynamic linking.** Would open up a few useful libraries. Rejected:
static linking is not optional for wasm and iOS, so we could not satisfy the relinking
requirement even if we wanted to.

**Allow GPL and license burrow under the GPL.** Internally consistent, and would let us
use MuPDF and Poppler. Rejected: incompatible with App Store distribution, and it
restricts our users' freedom to use burrow inside their own proprietary work — a worse
outcome for a tool meant to be universally usable.

**Review licenses manually at release time.** Cheaper day to day. Rejected: by release
time a bad dependency is load-bearing. The whole value of the constraint is that it binds
early.

**Allow anything and rely on the "on-device software" argument.** Rejected: it is wrong
for the GPL, unfalsifiable for the LGPL under static linking, and leaves downstream
distributors carrying our legal risk.
