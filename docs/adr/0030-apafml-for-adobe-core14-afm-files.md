# 0030. APAFML joins the allowlist, for Adobe's Core 14 AFM files

Date: 2026-10-09

## Status

Accepted. Amends [0003](0003-permissive-licensing.md) and
[0008](0008-widened-licence-allowlist.md) (as amended by [0010](0010-harfbuzz-and-icu-in-pdfium.md)
and [0012](0012-ncsa-for-libfuzzer.md)), which is the required form: [CLAUDE.md](../../CLAUDE.md)
says adding a licence needs an ADR, not a config edit. The owner approved the licence on
2026-10-09, in #290's slice.

## Context

#290 is a redaction leak in ordinary input. A standard-14 font with no `/Widths` takes its glyph
advances from metrics every viewer bundles and no file repeats, and burrow's table of those
metrics (`core/burrow-engines/src/pdfsyntax/standard14.rs`) was keyed by **code**, for codes 32 to
126, under two base encodings. A font whose `/Differences`, base encoding or descriptor flags
select a different glyph drew it at a width burrow did not compute, and the redaction returned
`Ok` with the secret still in the region.

The owner's decision (2026-10-09) is to key the table by **glyph name**, and to accept a name only
where the published Adobe metrics and PDFium's bundled faces agree, measured per style. The
owner's second condition: the table is **generated** from the AFM files and from measurements of
the pinned PDFium, never edited by hand, and a check regenerates it and diffs it against the
committed copy. That needs the AFM files in the tree.

The existing table was transcribed by hand from the published values. Before anything replaced it,
it was diffed against these files for every code in 32..=126, in all twelve styles, and the two
WinAnsi overrides: 1,156 pairs, **0 mismatches**.

### The licence

Adobe distributes the Core 14 AFM files as one archive, `Core14_AFMs.zip`, whose fourteen `.afm`
files are accompanied by `MustRead.html`. That file's text, verbatim:

> This file and the 14 PostScript(R) AFM files it accompanies may be used, copied, and distributed
> for any purpose and without charge, with or without modification, provided that all copyright
> notices are retained; that the AFM files are not distributed without this file; that all
> modifications to this file or any of the AFM files are prominently noted in the modified
> file(s); and that this paragraph is not modified. Adobe Systems has no responsibility or
> obligation to support the use of the AFM files.

SPDX lists this as **`APAFML`** (Adobe Postscript AFM License).

It passes ADR 0008's admission test:
- It is permissive: any purpose, no charge, modification permitted.
- It is not copyleft.
- It has no field-of-use or non-commercial restriction.
- Its obligations are notice and attribution only: keep the copyright notices, ship the notice
  file with the AFMs, and mark any modification.

Passing that test makes a licence eligible; this ADR is what admits it.

## Decision

We will add `APAFML` to the allowlist, **for Adobe's Core 14 AFM files only**. It is not a
general-purpose addition. A different file under the same identifier needs its own ADR.

We will keep the obligations by construction rather than by care:

- `third_party/adobe-core14-afm/` holds the fourteen AFM files and `MustRead.html` **exactly as
  published**, byte for byte. The provenance record sits beside the directory, not in it
  (`third_party/adobe-core14-afm.toml`): the pinned URL, the archive's sha256, and every file's.
- `tools/check-afm-provenance.sh` refuses the tree if a recorded file is missing, an unrecorded
  file is present, the notice is not among the files, or any file's bytes differ from the
  published ones. So we never ship an AFM without its notice, and never modify one. With
  `--fetch` it re-downloads the archive from the pinned URL and compares every file byte for byte.
  Its self-test plants each defect and requires the refusal by name.
- The files are read only by the test that generates
  `core/burrow-engines/src/pdfsyntax/standard14_table.rs`. The generated table carries data
  derived from them (glyph names, advance widths, and the StandardEncoding code each AFM assigns
  over 32..=126), and a header saying it is generated output.
  Nothing in a shipped build parses an AFM.
- `THIRD_PARTY_NOTICES.md` records the files, the licence and the notice text.
- **The derived data ships, and is treated as a modified extract** (the license-auditor's
  reading, the stricter of two). The generated table carries glyph names, widths and StandardEncoding codes, not AFM files,
  and whether that is a modification of the AFMs or uncopyrightable fact is not ours to settle, so
  we keep the obligation either way: its header carries every used AFM's Notice line and the
  accompanying paragraph verbatim, and says prominently that it is a modified extract and of what.
  It ships in the redaction wasm module only. **Before `/redact-pdf` reaches visitors (#136), Adobe
  needs an entry on `/credits` and the apps' licence screens** -- a Rust comment does not survive
  into a wasm binary. `/credits` is generated from `engines/licenses.toml`, which has no slot for
  data files, so that is a small new mechanism, and it changes what visitors see: the owner's call,
  at #136.

## Consequences

- The standard-14 table can cover every glyph name the fonts carry, not just ASCII, and it can be
  regenerated rather than retyped. That retyping is what produced the `DISPUTED` rows: names
  where PDFium's bundled face disagrees with the published metrics.
- We carry 0.6 MB of third-party data that must never be edited. A tidy-up, a line-ending
  normalisation or an editor's trailing-whitespace pass would breach the licence. The check makes
  any of those a red build rather than a silent breach.
- Upstream is frozen (the files date from 1997–2000), so there are no updates to track. The pinned
  mirror is Adobe's own legacy download host. If it disappears, the committed copy and its
  checksums remain authoritative; only `--fetch` stops working.

## Alternatives considered

- **pdf.js's metrics tables, under Apache-2.0** (already allowed). Rejected by the owner's choice:
  they are a transcription of the AFMs, not the AFMs, and the condition was generation from the
  source.
- **Keep transcribing by hand.** It departs from the owner's condition that the table be
  generated, and a 315-names-per-style transcription is where typing errors live.
- **Fetch at build time instead of committing.** That would make every build, CI included, depend
  on a legacy download host. The checksums would still pin the bytes, but availability would not
  be ours. Committing the unmodified files with their notice is what the licence permits, and the
  check proves we did it unmodified.
