---
name: m1-split-pruning
description: Measured limits of split's #54 pruning (prune.rs + pdfsyntax) — the image-XObject refusal, the nested-OC bypass, the /Resources category denylist and the per-page re-decode DoS
metadata:
  type: project
---

Reviewed 2026-09-14 on branch `split-pruning-harness` (ADR 0019's amendment). All five below
were **reproduced** with a scratch crate depending on `core/burrow-ops` by path
(`features = ["native-engines"]`, `LD_LIBRARY_PATH=engines/vendor/native-aarch64/lib`),
calling `burrow_ops::split` on hand-built PDFs. There is no `qpdf` CLI on this machine —
decompress output with `zlib.decompress` over `stream`…`endstream` spans in python instead.

1. **`absorb_streams_under` decodes every `/XObject` entry, including images.** It only
   checks `type_code == STREAM`, never `/Subtype /Form`. A Flate image's pixel bytes are
   lexed as content: an unbalanced `(` gives
   `Malformed("pdf syntax: a '(' string that is never closed")` and the split fails. A
   `/DCTDecode` image is never `filtered` at `qpdf_dl_specialized`, so `stream_data` returns
   `None` → `Unsupported("a stream on this page could not be decoded…")`. **No conformance
   fixture contains an image at all**, which is why nothing caught it.
2. **The optional-content refusal is page-level only.** `refuse_optional_content` reads the
   page's own `/Resources /Properties` and `/XObject` `/OC`. Put the OCG in a *Form
   XObject's* `/Properties` and the split succeeds, emitting the hidden content plus the
   OCG's `/Name` with no `/OCProperties` — ADR 0019 §2a row 6, reproduced.
3. **`prune_resource_dictionary` is a denylist of seven categories.** A non-category key on a
   shared/inherited `/Resources` (e.g. `/Stash`) is never examined and rides into every
   output with its excluded-page payload. Same shape the page-key allowlist exists to avoid.
4. **Over-prune:** names used inside a nested Form XObject that has no `/Resources` of its
   own are missed (the fixpoint looks names up only in the *page's* categories), so a font
   the page genuinely draws with is deleted. Reproduced: `/Font << >>`.
5. **DoS.** `prune_output` takes no deadline (only `annots_sharing` gained one), and
   `used_names`' visited set is per page, so a shared stream is re-decoded once per page.
   Measured: a **34 KB** file, 100 pages sharing one 20 MB-inflating form XObject → **17.4 s**;
   1000 pages (155 KB) → **176 s**, RSS 63 MB. `max_pages` is 10,000 and `split`'s deadline is
   checked only *between* outputs, so a one-output split has no checkpoint at all.

**How to apply:** redaction reuses `prune.rs` and `pdfsyntax/` unchanged (ADR 0019's transfer
table), so re-check 1–4 there before trusting the module. See [[m1-split-copy-semantics]] for
what the copy carries in the first place and [[m1-limits-real-strength]] for why RSS never
fires on 5.
