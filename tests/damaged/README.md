# Damaged-document regression fixtures

Deliberately **not** `tests/conformance/fixtures/`. That directory is the differential corpus:
every file in it has a hand-written expected outcome in `expectations.json`, three readers
compare against it, and `the_corpus_is_not_shrinking` gates its size. Adding to it changes a
measurement the project relies on.

These are something narrower — committed reproductions of defects and of the refusals that
answer them, each named in the table below with the tests that read it.

**The bar for adding one.** Damaged input is exactly what finds engine defects, so every fixture
here is run under AddressSanitizer through every operation fuzz target before it is committed,
and the PR adding it records the result. After it lands, `tools/check-fixtures-survive.sh`
holds it, natively, to every shipped operation. A fixture here graduates to the corpus when its defect is fixed and there is an
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
| `xref-entry-past-end.pdf` | 1,454 bytes. A minimal document with one in-use cross-reference entry past the end of the file; nothing else is wrong with it. The prescan refuses it before any engine opens it (`prescan::check`, "a cross-reference entry places an object past the end of the file"). Read by `optimistic_counts.rs` (every operation refuses it by the prescan) and the redaction golden file. |

`page-loss-on-write.pdf`, #61's reproduction: replaced a damaged-fixture test; see #259.

Every file here is also read by `tools/check-fixtures-survive.sh` (#260), which runs each tracked
fixture through every shipped operation with its outcome pinned in `tests/fixtures-survive.tsv`.
