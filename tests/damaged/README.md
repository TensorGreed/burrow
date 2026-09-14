# Damaged-document regression fixtures

Deliberately **not** `tests/conformance/fixtures/`. That directory is the differential corpus:
every file in it has a hand-written expected outcome in `expectations.json`, three readers
compare against it, and `the_corpus_is_not_shrinking` gates its size. Adding to it changes a
measurement the project relies on.

These are something narrower — committed reproductions of defects, each named by the issue it
belongs to, used by a single `#[ignore]`d test that states a gap rather than asserting a
behaviour. A fixture here graduates to the corpus when its defect is fixed and there is an
outcome worth pinning.

## The input class, and why none of it was covered before

Every damaged fixture in the conformance corpus is damaged enough that qpdf refuses to open it
— `truncated.pdf`, `trailer-removed.pdf`, `truncated-mid-object.pdf`, `not-a-pdf.bin`. A
refusal is a typed error and the caller is told; that is the easy case, and it is the only one
that was tested.

The narrow band between "opens cleanly" and "refused" was not covered at all, and it is hard to
reach by construction. Four attempts at generating one landed in the refused class instead:
corrupting an xref offset, widening a `/Kids` reference, routing the bytes through
`String::from_utf8_lossy` (which replaces the binary comment on line 2 and moves `startxref`),
and pointing a branch at another branch. Fuzzing found it in minutes.

## Files

| file | what it is |
|---|---|
| `page-loss-on-write.pdf` | 1,871 bytes. A two-level, six-page tree with one page object's cross-reference entry pointing past the end of the file. burrow opens it and reports **5** pages; writing it out — unedited, through the identity permutation, or through `rotate` — yields a valid PDF with **4**. No error on any path. Found by the `reorder` fuzz target, 2026-09-13. |

`page-loss-on-write.pdf` is clean under AddressSanitizer (checked against the `qpdf_check`
target); it reproduces data loss, not memory unsafety.
