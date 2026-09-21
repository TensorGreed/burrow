---
name: m2-web-handle-newtype
description: web/handle.rs's WebHandle (commit b887eff, 2026-09-21) — measured: it does NOT bind to its Session the way native ObjectHandle does, and its scan test is blind to `const fn` and rustfmt-wrapped signatures.
metadata:
  type: project
---

Measured 2026-09-21 on branch `m2/web-handle-newtype`, commit `b887eff`, which folded five web
qpdf files onto one `WebHandle` with one `Drop`.

**The release side is correct.** Walked every path: `declared_rotation`'s `current = parent`
drops the child exactly once per iteration and the last one at function exit; `permute`'s
`Vec<WebHandle>` and per-iteration `current` are distinct qpdf ids so no double release;
all six `WebHandle::owned` call sites pass a freshly-issued handle; all four surviving
`.raw()` sites pair with the right document. Drain counts are one-for-one with the
`session.take_error()` calls they replaced (`Session.bridge` is `Arc::clone(&engine.bridge)`,
so `WebHandle::take_error` reaches the same object). 209 lib tests green, clippy clean.

**The gap that matters: `WebHandle<'a>` borrows the ENGINE, not the Session.** Native
`qpdf/handle.rs` has `owner: PhantomData<&'a Document>` and `owned(document: &'a Document, …)`.
The web twin takes a bare `QpdfPtr` and only `&'a WebQpdf`, so a handle may outlive the
`Session` whose `Drop` calls `qpdf_cleanup`. **Proven by compiling** a function that opens a
local `Session`, takes `rotate::page_handle` over it, and returns the handle — builds with no
error. `extract` already has that shape (local `dest` Session + `blank` handle), so the next
edit there is one reordering away from `oh_release` on a freed `qpdf_data`.

**Two ways past `no_method_takes_a_handle_and_a_separate_document`, both measured by planting
real methods and watching the test stay green:**
- the filter matches `pub(super) fn ` / `fn ` on a *trimmed line*, so `pub(super) const fn` is
  invisible — and three of the type's own methods (`owned`, `raw`, `sibling`) are `const fn`,
  so that is the house style. `owned`'s docstring claims it is "the exception and is named";
  it is not named anywhere, it is excluded by the `const` keyword.
- a rustfmt-wrapped signature puts `&self` and `QpdfPtr` on lines the filter never sees.

Also: the probe closure re-implements the rule instead of calling it, so a regression in the
real filter cannot fail the probes; and `replace_key(&self, key, item: &Self)` uses `self.data`
with `item.handle`, which is the #130 cross-document shape expressed through `&Self` rather
than `QpdfPtr`, so the exception list cannot see it.

See [[m1-web-reorder-bridge]] for the `Pages` guard this replaced and [[m1-web-rotate-bridge]]
for the ancestor walk's earlier holes.
