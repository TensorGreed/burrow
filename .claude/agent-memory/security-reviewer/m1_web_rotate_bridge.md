---
name: m1-web-rotate-bridge
description: The web rotate bridge (branch m1-rotate-bridge, 2026-09-13) — why both rotate conformance cases are satisfiable by the ancestor-write bug, and where the hand-release discipline actually leaks.
metadata:
  type: project
---

Measured 2026-09-13 on branch `m1-rotate-bridge` (uncommitted), the web half of `rotate`.

**Both rotate conformance cases pass under the bug they exist to catch.** `Operation::Rotate`
is fixed at "every page, by 90", and neither fixture gives any page its own `/Rotate`:

- `pages-10.pdf` — flat tree, no `/Rotate` anywhere, expected `[90; 10]`.
- `inherited-rotation-6page.pdf` — object 2 is `/Type /Pages /Kids [3 0 R 4 0 R] /Count 6
  /Rotate 90`; objects 3 and 4 are intermediate `/Pages` with no `/Rotate`; the six pages have
  none. Expected `[180; 6]`.

An implementation that wrote `/Rotate` onto the shared root instead of onto each page produces
**exactly** those two expected vectors, because every page still inherits. That is hazard #2 in
`qpdf/rotate.rs`'s own header ("the write goes on the page, never on the ancestor it was read
from") — the bug that rotates the whole document while reporting success. Because the operation
is fixed at every-page, **no corpus case can ever distinguish it**, whatever fixture is added.
The native path has a unit test for it
(`rotating_one_page_leaves_every_other_page_alone_including_ones_that_inherit`); the web path,
as of this branch, has none.

**`core/burrow-engines/src/web/tests.rs` was not touched.** `cargo test -p burrow-engines` runs
96 unit tests and none of them names rotate. `web/rotate.rs`'s module header nonetheless claims
"`web/tests.rs` counts … a test asserts the count is zero after a rotation and after a refusal".
The fake's new instrumentation — `QpdfScript::live_handles`, `oh_type_codes`, `oh_int_value`,
and the `Call::OhGetKey/OhReplaceKey/OhRelease` variants — is written and read by nothing.
Clippy does not flag it (constructed by `Default`).

**The hand-release discipline has exactly two holes**, both `copy_key(...)?` returning past a
live handle: `web/rotate.rs:251` (`value` from `oh_new_integer` is live) and `:355` (`current`
is live, inside the `/Parent` walk). Everything else — including every `take_error` branch,
both `oh_get_key` error paths and the depth-exhausted exit — releases. Both holes need a null
from the engine `malloc`, which with `-sALLOW_MEMORY_GROWTH=1 -sMAXIMUM_MEMORY=2GB` is
reachable at the 2 GiB cap rather than being hypothetical; impact is bounded because
`qpdf_cleanup` clears the whole `oh_cache` on `Session::drop`. The engine-heap `free(key)`
pairing is leak-free on every path (no `?` between `copy_in` and `free`).

**The cheaper shape** for the per-node key allocation: hoist `/Rotate` and `/Parent` into the
`Session` (one `copy_in` each at open, freed in `Drop`). At `max_pages` 10,000 with a 63-deep
tree the current shape is ~1.3 M malloc/free pairs, and hoisting removes both leak holes by
making the walk infallible on allocation.

**`page_rotations` (bindings/burrow-wasm/src/lib.rs) is the one entry point that drives an
engine trait directly**, bypassing `burrow-ops`. Its `for page in 0..pages` loop has no
deadline checkpoint — `PageRotator::effective_rotation` takes no clock — so `max_duration_ms`
is applied once, in `open`, and never again across pages × up-to-64 ancestors. What actually
stops a long run is `worker-host.js`'s terminate, not the configured limit, so the caller never
sees `LimitExceeded`.

**Clean, and re-verified rather than assumed:** all six `qpdf_oh_*` pass
`tools/check-qpdf-trapped.py` (four `via do_with_oh -> trap_oh_errors`, `oh_new_integer` and
`oh_release` argued in `qpdf-untrapped-accepted.toml`); `tools/check-wasm-exports.sh` passes
with `qpdf_remove_page` as the only declared-not-exported entry; no JS bridge function branches
on engine state; no key can be derived from document content (both are Rust consts, copied in,
and JS only forwards the pointer); nothing new reaches the network or a log.

See [[m1-qpdf-exception-boundary]] for the trapped-set mechanisation's own limits, and
[[m1-differential-harness]] for what `compare.ts` binds.
