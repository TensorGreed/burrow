# 0017. Which engine merges, and what a merge does when one input fails

Date: 2026-09-13

## Status

Accepted.

## Context

`merge` is M1's first operation and the first use of the `add-operation` checklist end to
end. Three questions had to be settled before any of it could be written, and all three
were open in a way a function signature could not decide.

**Which engine does the work.** Both can. PDFium has `FPDF_CreateNewDocument`,
`FPDF_ImportPagesByIndex` and `FPDF_SaveAsCopy`; qpdf's C API has `qpdf_add_page`,
`qpdf_init_write_memory`, `qpdf_write` and `qpdf_get_buffer`. Nothing in the existing ADRs
chooses between them, because until now no operation produced bytes — every engine
capability in the crate was read-only (`DocumentEngine` is `{ open, page_count }`,
`StructureEngine` is `{ check }`, and `StructureReport` has one field).

**What happens when one of several inputs fails.** Merge is the project's first operation
with more than one input, so "the operation failed" stops being the only possible answer.

**Whether `Limits` apply per input or to the total.** Same reason: every ceiling in `Limits`
was written for one document.

## Decision

### 1. qpdf merges. PDFium does not.

**Measured, not assumed.** `core/burrow-engines/examples/measure-merge.rs` merges a set of
fixtures through both engines' C APIs directly and reports what came out. Each fixture
carries a literal `BURROWMARK` inside the feature under test — an outline title, an
annotation's `/Contents`, a form field's `/T`, an embedded file's bytes — so "did it
survive" is answered by looking for that marker in the output after `qpdf --qdf` has
decompressed it, rather than by a structural walk that would be a second parser to trust.

What survives a two-document merge:

| | PDFium | qpdf |
|---|---|---|
| outline / bookmarks | **lost** | preserved |
| annotations | preserved | preserved |
| form field, `/AcroForm` | **lost** — the widget annotation survives, the form dictionary does not | preserved |
| attachments, `/Names /EmbeddedFiles` | **lost** | preserved |
| `/CropBox`, `/Rotate` | preserved | preserved |
| page count and order | correct | correct |
| output size, ordinary fixtures | larger in 5 of 7 | smaller in 5 of 7 |

**The `/AcroForm` row is the one that decided it, and it is worse than the word "lost"
suggests.** PDFium keeps the widget annotation and drops the form dictionary, so the merged
document shows a form field that is not a field: it looks filled-in and nothing can read
its value. A feature that disappears is a disappointment; one that appears to be there and
is not is the failure class this project treats most seriously, and the reason
non-negotiable #4 puts correctness before features.

Losing bookmarks and attachments is not a rounding error either. Someone merging a report
and its appendix is merging exactly the kind of document that has an outline.

**PDFium is more tolerant of damaged input, and that is not a reason to choose it.**

| input | PDFium | qpdf |
|---|---|---|
| `truncated.pdf` | refuses | refuses; cannot even count pages |
| `trailer-removed.pdf` | refuses | **counts 3 pages, `qpdf_add_page` then fails** |
| `encrypted.pdf`, no password | refuses, code 4 (password) | refuses |
| `xref-bomb.pdf` | **merges, 3 pages out** | counts `-1`, `add_page` fails |
| `object-number-above-int-max.pdf` | **merges** | counts 1 page, `add_page` fails |

The last two rows are PDFium succeeding on files this project already refuses: the
structural pre-scan (ADR 0013) rejects `xref-bomb.pdf` before any engine allocates, and
`object-number-above-int-max.pdf` is the 356-byte file that used to abort the process.
Neither reaches an engine under `Limits::default()`, so PDFium's tolerance buys nothing a
caller can use.

**ADR 0013's repair finding does not carry over to merge, and this is the correction worth
recording.** That ADR establishes that qpdf reads a truncated file (1 page) and a
trailer-less one (3 pages) that PDFium refuses outright — and it is right, because what it
measured was `qpdf_get_num_pages`. A merge needs `qpdf_get_page_n` and `qpdf_add_page` on
top of that, and those are a strictly higher bar: on `trailer-removed.pdf`, qpdf counts
three pages and then fails to extract the first one. **Counting pages in a damaged file is
not the same capability as merging them**, and a plan that assumed otherwise would have
promised repair behaviour no engine here delivers.

### 2. Any failing input fails the whole operation

No partial success. If one of five inputs cannot be read, no output is produced, and the
error names **which input** failed and why.

A merged PDF that silently omits an input **looks complete**. There is no page that says
"something was dropped here", the page count is plausible, and the person discovers it when
they need the missing pages. That is data loss wearing the costume of success, and it is
the same shape as the `/AcroForm` finding above: the failure mode this project refuses is
not "it broke", it is "it looks like it worked".

The alternative — merge what can be merged, return a per-input outcome list — was
considered and rejected. It is genuinely friendlier for someone merging twenty scans, and
it has one property that is not recoverable: a caller who ignores the list ships a short
document and never finds out. Every binding, every UI and every future caller would have to
get that right, forever, to avoid an outcome the all-or-nothing rule makes impossible.

The cost is real and is accepted: a person with one bad file in twenty gets nothing until
they remove it. The tool page is obliged to make that easy — it marks the offending file in
the list, says what is wrong with it in a sentence, and leaves the others in place.

### 3. Limits apply per input **and** to the total

Per input alone is a hole, and an obvious one: a hundred inputs each a byte under
`max_input_bytes` is a hundred times the ceiling, from a caller who set it.

| limit | per input | across the operation |
|---|--:|---|
| `max_input_bytes` | yes, at `Stage::InputSize` | yes, against the sum |
| `max_pages` | yes | yes, against the output total |
| `max_duration_ms` | — | the whole operation, on the injected clock, checkpointed between inputs |
| `max_memory_bytes` | detected, per input | detected |
| `max_pixels` | not applicable — merge decodes no raster | — |

`max_memory_bytes` keeps the meaning ADR 0007's 2026-09-12 amendment gives it: **detected,
not bounded**, on every path. Merge does not change that and must not be documented as if
it did.

The rustdoc on `merge` states this table. A caller who reads `max_input_bytes` and assumes
it is per file would size their ceiling wrongly in the direction that hurts.

## Consequences

**The crate grows its first output-producing engine capability**, and it is qpdf's. That
means new `unsafe extern "C"` declarations in `qpdf/ffi.rs`, a new trait method, new bridge
methods on `QpdfBridge` with their `__burrow_*` imports and `web/fake.rs` `Call` variants,
and a reply in `burrow-wasm` that can carry bytes.

**Two of the functions needed are not on `engines/qpdf-trapped-functions.txt`:**
`qpdf_get_buffer` and `qpdf_get_buffer_length`. Each needs an argued entry in
`engines/qpdf-untrapped-accepted.toml` meeting that file's stated bar — non-parsing, reads
a stored value — or a different route. They are accessors on a buffer qpdf has already
produced, which is the shape that bar admits, but the argument has to be written and
checked rather than assumed.

**The wasm qpdf module must export more than it does.** `engines/build-wasm.sh`'s
`qpdf_exports` is a deliberate allowlist mirroring `qpdf/ffi.rs`, and none of the page or
write API is in it. Extending it means rebuilding the pinned wasm artifact, new content
hashes, a regenerated CSP, and a size-budget re-measure. That is the acquisition route
working as designed — the allowlist is supposed to make this a decision — and it is the
concrete cost of choosing qpdf over PDFium, which needed no engine-build change at all.

**PDFium's wasm module could have done this with no build change, and its function table is
growable.** Measured from the modules' own table sections: PDFium's has no maximum, so
`addFunction` works and `FPDF_SaveAsCopy`'s `FPDF_FILEWRITE` callback would have been
reachable; qpdf's is fixed at 2047 and cannot grow. That does not affect merge — qpdf's
write path fills an internal buffer and needs no callback — but it is worth recording
before someone reaches for a qpdf API that takes a function pointer.

**Three ordering and status constraints, each of which cost something to find**, and each
of which the implementation must respect:

- `qpdf_set_static_ID` and the other write parameters must be called **after**
  `qpdf_init_write_memory`, not before. `qpdf-c.h` says so; calling it first dereferences a
  writer that does not exist and takes the process down. Found by core dump.
- `qpdf_read_memory` returns a **bitmask** (`QPDF_WARNINGS` is bit 0, `QPDF_ERRORS` is bit
  1). Testing `!= 0` treats a warnings-only read as a failure, and every damaged file in
  this corpus reads with warnings. `qpdf/mod.rs` already says this; the measurement harness
  reproduced the bug anyway and reported qpdf refusing two files it does not refuse.
- **The error slot must be drained before `qpdf_cleanup`**, or qpdf prints
  `WARNING: application did not handle error: …` to stderr — **outside the logger**, so
  `qpdf_silence_errors`, `qpdf_set_suppress_warnings` and a discarding logger do not stop
  it. All three were installed and the line still appeared. `qpdf/mod.rs`'s `Drop` drains
  first for exactly this reason.

**A merged document is a new document, and its metadata is the first input's.** qpdf's
`addPage` copies pages into the destination, which is the first input read; the outline,
`/AcroForm` and `/Names` that survive are that document's. Pages from later inputs arrive
without theirs. This ADR does not try to merge outlines from several documents — that is a
feature, it has no obviously right answer, and inventing one here would be deciding it by
implementation. It is stated so nobody reads the fidelity table as a promise about input
two.

## Alternatives considered

**PDFium, for the tolerance and the single code path.** It is already wired into the engine
thread, its wasm module needs no rebuild, and it merges files qpdf will not. Rejected on
the `/AcroForm` finding: an operation whose output silently contains a dead form field is
not one this project should ship, and the files it uniquely handles are ones the pre-scan
refuses anyway.

**Both, with PDFium as a fallback when qpdf refuses an input.** Tempting, and it is the
shape ADR 0013 uses for structure. Rejected for now: the fallback would produce output with
different fidelity depending on which engine handled it, from the same API call, with
nothing in the result saying which. A merge that sometimes keeps your bookmarks is worse to
reason about than one that always does or always does not. If a corpus run later shows a
population of files qpdf refuses and PDFium merges correctly, this is the thing to revisit,
and the divergence must be visible in the typed outcome.

**Partial success**, covered in §2.

**Per-input limits only**, covered in §3.
