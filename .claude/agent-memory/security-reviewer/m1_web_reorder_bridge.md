---
name: m1-web-reorder-bridge
description: The web reorder bridge (branch m1-reorder-bridge, 2026-09-14) — why the Pages guard is actually leak-free, why the oh_object fail-open drain is sound, and the self-test that emptying qpdf-not-exported.toml breaks.
metadata:
  type: project
---

Measured 2026-09-14 on branch `m1-reorder-bridge` (uncommitted), the web half of `reorder`.

**The `Pages` guard is genuinely leak-free — unlike rotate's hand-release.** Re-walked every
`?` in `core/burrow-engines/src/web/reorder.rs`. `permute`'s collection loop `?`s with
`originals` already live (partial vector drops); `current` is taken after the guard and
released via the `let outcome = …; oh_release; outcome?;` shape so no `?` returns past it;
`move_into_place` takes no handles at all. `page_handle` releases on its own error path.
Rotate's two holes (`rotate.rs:251`, `:355`) have no analogue here. This shape is worth
copying, not the rotate one.

**`trap_errors` never CLEARS `qpdf->error` — it only sets it on exception** (qpdf-c.cc:69-89;
only `qpdf_get_error` consumes the slot). That is what makes the `oh_object` fail-open safe:
`__burrow_qpdf_oh_object` makes two calls, and a failure in the first survives a success in
the second, so the single `take_error()` after both in `move_into_place` catches it. Do not
re-derive this — it is the load-bearing fact behind the whole two-call packing.

**`(0,0)` from `oh_object` is not reachable for a page handle.** `getObjectID()` returns 0 for
a *direct* object, but `Pages::cache()` (QPDF_pages.cc:~157) rewrites every direct `/Kids`
entry with `makeIndirectObject`, resolves duplicate pages by shallow-copying, and
`pushInheritedAttributesToPage` calls `cache()` first. So no crafted page tree yields two
pages that both read (0,0) silently.

**Depth is bounded at 100 by `getAllPagesInternal`'s `max_level`**, and
`pushInheritedAttributesToPageInternal` recurses with *no* loop detection of its own —
relying entirely on having been preceded by `cache()`. `util::assertion` throws
`std::logic_error`, which `trap_errors` catches via `catch (std::exception&)`. No abort path.
Inherited non-scalar resources are made indirect and *shared*, so flattening does not amplify
output size.

**Emptying `engines/qpdf-not-exported.toml` NO LONGER breaks `tools/test-check-wasm-exports.sh`.**
It did, and it was fixed: RULE 4 used to build its fixture by deleting the last entry's
`reason` from the REAL file (`rpartition` + `assert sep`), which threw on a file with zero
entries and left the case running against a nonexistent path. It is now self-contained --
it writes its own ffi.rs and its own entry. **Re-verified 2026-09-15 on `split-bridge`, where
the file really is empty (0 `[[function]]` entries)**: rules 1/2/3/5 append synthetic entries
to whatever is there, so all five still fire. The exemption mechanism again has no live
instance in the repo, which is the lifecycle working, not a gap.

**`check-wasm-exports.sh` fails locally on any branch that adds an export** until
`engines/build-wasm.sh` is re-run — the committed `engines/vendor/wasm/lib/qpdf.wasm` is
stale. CI is fine: the wasm cache key is
`hashFiles('engines/pins.toml','engines/fetch.sh','engines/build-wasm.sh')`, so editing the
export list forces a rebuild.

**rotate's worker validation is weaker than reorder's**, not the other way round:
`main.js`'s rotate branch does `(request.pages ?? []).every(…)` with no `Array.isArray`, so a
non-array `pages` throws a TypeError. reorder added the guard.

See [[m1-web-rotate-bridge]] for the rotate comparison, [[m1-qpdf-exception-boundary]] for
the trapped-set mechanisation's limits.
