# 0008. Widened licence allowlist for bundled native engines

Date: 2026-09-10

## Status

Accepted — supersedes [0003](0003-permissive-licensing.md).
**Corrected and amended by [0010](0010-harfbuzz-and-icu-in-pdfium.md).**

Two errors in this ADR's record, found when M1 PR 1 audited the native artifacts:

- The component table below says **ICU is not linked**, reasoning from
  `pdf_enable_xfa = false`. That reasoning is wrong — ICU arrives through HarfBuzz's
  `hb-icu` integration, and 489 of its symbols are in `libpdfium.so` with RTTI evidence
  in `pdfium.wasm` too. ADR 0010 corrects it.
- **HarfBuzz is missing entirely.** It is linked, and PDFium's package ships no licence
  file for it, so this ADR's allowlist was incomplete rather than merely narrow.

The decision and reasoning below stand as written.

## Context

[ADR 0003](0003-permissive-licensing.md) set a permissive-only licence allowlist and made
`cargo-deny` enforce it. That was right, and its reasoning still stands: burrow ships
statically linked binaries and a WebAssembly module, so copyleft is unusable, and we will
not accept a dependency whose licence we cannot determine.

But that list was drawn up while the dependency graph was Rust crates. The M1 spike
([spike 0001](../spikes/0001-wasm-engines.md)) audited the actual native engines for the
first time, and the list turns out to be too narrow to describe a bundled C/C++ PDF
engine. PDFium's WebAssembly package fails it on four counts:

| Licence | Component | Status |
|---|---|---|
| `FTL` (FreeType Project Licence) | freetype | **Confirmed linked** into `pdfium.wasm` (`FREETYPE_PROPERTIES`, `tt-glyf`, `tt-cmaps` symbols) |
| `IJG` | libjpeg-turbo's libjpeg API half; also libjpeg 9f via Emscripten's port | **In both `pdfium.wasm` and `qpdf.wasm`** |
| AGG 2.3 | agg23 | Bespoke grant, **no SPDX identifier exists** |
| `libpng-2.0` | libpng | Shipped in the package; not confirmed in the wasm |

Three facts shape the decision.

**None of it is copyleft.** The audit grepped the whole tree. The only GPL text found was
in ICU's licence file, covering four ICU4C autotools files (`aclocal.m4`, `pkg.m4`,
`config.guess`, `config.sub`), each carrying the Autoconf exception and none compiled.
This is not the Ghostscript/Poppler problem ADR 0003 was written to prevent.

**None of it threatens our own licensing or distribution.** All four are permissive in
substance. Their only obligations are notices and credit lines. `MIT OR Apache-2.0` for
burrow itself is unaffected, and App Store distribution stays open.

**Rebuilding from source would not help.** PDFium bundles these components regardless of
how it is built, so this is not a consequence of using a prebuilt binary and cannot be
avoided by choosing a different acquisition route.

The alternative to widening the list is dropping PDFium, which means reopening ADR 0004's
engine selection — and the permissively licensed alternatives were already rejected there
on capability grounds, more sharply for redaction in M2.

## Decision

### The allowlist

We accept dependencies under these licences, and no others:

**From ADR 0003, unchanged:** MIT, BSD-2-Clause, BSD-3-Clause, Apache-2.0 (including WITH
LLVM-exception), ISC, Zlib, MPL-2.0, OFL-1.1, Unicode-3.0, CC0-1.0, Unlicense.

**Added by this ADR:**

| Licence | SPDX | Why |
|---|---|---|
| FreeType Project Licence | `FTL` | Font rasterisation in PDFium. Permissive; obligations are a credit line and a name-use restriction |
| Independent JPEG Group | `IJG` | JPEG decode in PDFium and qpdf. Permissive; obligation is a documentation credit |
| PNG Reference Library v2 | `libpng-2.0` | libpng in PDFium. Permissive, notice-only |
| Anti-Grain Geometry 2.3 | **`LicenseRef-AGG-2.3`** | Vector rasterisation in PDFium. No SPDX identifier exists, so we name it ourselves |

`LicenseRef-AGG-2.3` is a local identifier, in the form SPDX reserves for exactly this.
The full grant is committed at [`licences/LicenseRef-AGG-2.3.txt`](licences/LicenseRef-AGG-2.3.txt)
so the thing we accepted is in the repository rather than described from memory. **Version
2.3 specifically**: AGG 2.4 and later were relicensed to the GPL, so the version pin is
load-bearing, not incidental.

**Still forbidden, unchanged:** GPL (any version), LGPL (any version), AGPL, SSPL, any
non-commercial or field-of-use restriction, and anything whose licence cannot be
determined. Unclear remains forbidden, not deferred.

Note especially: **GnuTLS is LGPL-2.1-or-later**, and qpdf will link it by default —
see ADR 0004 for the flags that prevent it and the CI assertion that enforces them.

### The admission test

A licence may be *proposed* for the allowlist if all of the following hold. This exists so
future decisions are argued against a written standard instead of relitigating ADR 0003
each time.

1. **Permissive.** A grant to use, modify, and redistribute, including commercially.
2. **No copyleft.** No obligation to license our work, or any part of it, under the same
   or a compatible licence — including per-file copyleft where it would reach our code.
3. **No field-of-use or non-commercial restriction.** Our users must be able to use burrow
   for anything, including inside proprietary work.
4. **Obligations limited to notices and credit lines.** Attribution, notice retention, and
   name-use restrictions are acceptable. Anything requiring source disclosure, relinking,
   patent retaliation beyond Apache-2.0's, or a per-user agreement is not.

**Passing the test does not add a licence to the list.** It makes a licence *eligible* to
be added by a new ADR. The test explains the policy; it does not bypass it. `deny.toml`'s
`exceptions` list stays empty, and the allowlist is still only editable by an ADR.

### Notice obligations, and how we meet them

Two of the added licences impose affirmative obligations that bind **executable-only**
distribution — which is exactly what we ship. These are distribution requirements, not
bookkeeping.

**FreeType, FTL §2:**

> Redistribution in binary form must provide a disclaimer that states that the software is
> based in part of the work of the FreeType Team, in the distribution documentation.

**Independent JPEG Group, LEGAL ISSUES condition (2):**

> If only executable code is distributed, then the accompanying documentation must state
> that "this software is based in part on the work of the Independent JPEG Group".

FTL §3 additionally forbids using the FreeType name for promotional purposes without
permission. We do not, and must not start.

We meet these in four places, and **all four must exist before a build containing these
components is distributed**:

| Where | Requirement |
|---|---|
| [`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md) | Full notice text and both credit lines, verbatim |
| The website | A credits page, reachable from the footer, carrying the same notices |
| Android app (M3) | An open-source licences screen |
| iOS app (M4) | An open-source licences screen |

A notices file in the repository is not sufficient on its own: both obligations say
*documentation accompanying the distribution*, and for an app store build that means
something the user can actually reach.

### Machine-readable engine licensing

`cargo-deny` sees Rust crates and nothing else. It cannot see PDFium, qpdf, or the
components they bundle — which is precisely where every finding in this ADR came from, and
it took a manual audit to surface them. That is not repeatable.

So the native engine licence surface gets its own manifest,
[`engines/licenses.toml`](../../engines/licenses.toml), recording for every component:
its name, version, SPDX identifier or `LicenseRef-`, where its licence text lives, whether
it is confirmed linked into a shipped artifact, and any notice obligation it carries.

`tools/check-engine-licences.py` validates the manifest against this ADR's allowlist and
**fails CI** on any licence not listed. It runs in the same job as `cargo-deny`, so the
two halves of the dependency graph are checked by the same gate.

This manifest is also the input for M1's SBOM. An SBOM covering only Rust crates would
omit 82% of the shipped bytes and be actively misleading — which is why
`.github/workflows/release.yml` still refuses to run.

## Consequences

PDFium becomes usable, which unblocks M1. The licence surface is now written down,
machine-checked, and auditable by someone who was not present for the audit.

The cost is a genuinely longer list, and a longer list is a weaker constraint. Four of the
eleven original entries were added at once, on the strength of a single engine. The
admission test exists to stop that becoming a habit: the next addition still needs an ADR
arguing against the four criteria, and "PDFium needed it" is not an argument that
generalises.

We now carry affirmative notice obligations for the first time. Previously every accepted
licence was satisfied by shipping notice text; FTL and IJG require credit lines in
user-reachable documentation. That is a small, permanent product requirement on three
surfaces — and a release that omits them is a licence violation, not an oversight.

Two engines are audited; five from ADR 0004 are not. HarfBuzz, jbig2enc (and its Leptonica
dependency), mozjpeg, libwebp, and libavif have not been through this. Expect at least one
of them to need another ADR — jbig2enc in particular is flagged "verify" in ADR 0004 and
is the most likely to surprise us.

The manifest is hand-maintained, so it can drift from what is actually linked. The CI
check validates that every declared licence is allowed; it cannot detect a component
nobody declared. Catching that still needs a periodic audit, and the honest position is
that the manifest is a floor, not a guarantee.

## Alternatives considered

**Drop PDFium and use a permissively licensed engine.** The only option that keeps ADR
0003's list intact. Rejected: it reopens ADR 0004's engine selection, where the
alternatives were rejected on capability grounds — and for M2's redaction the capability
gap is not acceptable.

**Add the four licences to `deny.toml` without an ADR.** What a hurried reading of ADR 0003
would produce, since `deny.toml` is a config file. Rejected: ADR 0003 explicitly says
changing the allowlist requires a new ADR. Making that edit invisibly is how a licensing
policy stops meaning anything.

**Grant per-crate exceptions in `deny.toml` instead of widening the list.** Keeps the
headline list short. Rejected as dishonest bookkeeping: these licences are genuinely
acceptable to us, so they belong in the list. An exception implies a tolerated violation,
and the exceptions list is empty by design.

**Widen the list to "anything OSI-approved and non-copyleft".** Removes the need for future
ADRs entirely. Rejected: it would admit licences with obligations we have not read, and the
whole value of the constraint is that someone has read each one. The admission test gives
most of the convenience without giving up the review.
