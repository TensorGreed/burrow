---
name: rustdoc-gate-private-items
description: A rustdoc gate without --document-private-items checks intra-doc links on public items only; plant the broken link in a private item to see what it misses.
metadata:
  type: feedback
---

When reviewing a gate built on `cargo doc -D warnings`, plant a broken intra-doc link in a
**private** item (for example a `//!` line in a non-`pub` module) as well as a public one. Without
`--document-private-items`, rustdoc does not resolve links on private items. On #174 (2026-09-24)
the planted private link passed, and `--document-private-items --keep-going` turned up 14 real
broken links, 10 of them in engine-gated modules. That is the same class the issue was filed for.

Related, measured the same day: rustdoc deletes and regenerates `target/doc/<crate>/` for every
crate it documents in the current run. So a stale `target/doc` can only satisfy a page-exists check
for crates the current invocation did not document, for example after the command was narrowed with
`-p`. Test that shape, not a plain rebuild.

**Why:** a page-exists or count gate only proves the build ran with the feature on. It does not
prove the links in private items were checked, and they are where most of the stale links were.

**How to apply:** use this on any rustdoc or doc-coverage check. Also see [[text-scan-gate-passes-dead-rule]].
