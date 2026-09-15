---
name: m1-web-split-bridge
description: The web split bridge (branch split-bridge, 2026-09-15) — the measured attacker-chosen free() in bridge.js's two owning functions, the sparse-array hole that defeats the part-completeness gate, and what the fixpoint DoS turned out NOT to be.
metadata:
  type: project
---

Reviewed 2026-09-15 on branch `split-bridge` (uncommitted): PR 3 of the split series, shared
`prune/` policy + `web/prune.rs` + `web/extract.rs` + ADR 0023's multi-output protocol.

**The one that matters: `__burrow_qpdf_oh_page_content` / `__burrow_qpdf_oh_stream_data` read
their out-params before checking `status`.** `trap_errors` (qpdf-c.cc:68-89) returns on the
throw path **without writing `*bufp`, `*len` or `*filtered`** — verified in
`qpdf_oh_get_stream_data`/`..._page_content_data` at qpdf-c.cc:1725-1766. The JS reads
`HEAPU32[bufp>>>2]` unconditionally and the `finally` frees it whatever `status` said.
Emscripten `_malloc(4)` does not zero. **Measured against the committed
`engines/vendor/wasm/lib/qpdf.{js,wasm}` under node** (loadable with
`instantiateWasm: (i,cb)=>WebAssembly.instantiate(bin,i).then(r=>cb(r.instance,r.module))`,
since `-sENVIRONMENT=web,worker` blocks the fetch path):

- scratch on entry after one round-trip: `[0x27b3c, 0x400]` — the previous call's values;
- `page_content(len = 0xC0FFEE)` then the 3-word `stream_data` error path reads
  `buf = 0xc0ffee` and calls `free(0xc0ffee)`. **The freed address is the decoded length of a
  content stream, i.e. chosen by the PDF.** `qpdf_oh_free_buffer` nulls `*bufp` on success, so
  page_content→page_content is mostly benign; the cross-function 2-word/3-word interleave is
  what makes it controlled, and it is the real call order (page content, then its streams).

The native twin `qpdf/handle.rs:328-373` initialises all three out-params — that is the
difference, and it exists because the status branch lives in JS at all (ADR 0009 §2: a binding
"may not interpret an engine error code").

**Dead end, measured, do not re-raise: `used_names`' fixpoint is NOT a DoS.** It looks O(N²)
in a page's `/XObject` count with no checkpoint on the cache-hit path, but each iteration
discovers one new name = one cache MISS = one `walk.deadline.checkpoint`. Chained-form fixtures
through the native `burrow_ops::split` (debug build, aarch64): N=50 14 ms, N=200 105 ms,
N=500 560 ms, **N=2000 fires `max_duration_ms` at 5.003 s of a 5 s budget**, N=5000 refused by
`pdfsyntax::dict::MAX_KEYS` ("a dictionary with more keys than burrow will read"). Bounded.

**Still open and NOT measured: `WebGraph::key_ptr` allocates per USE, not per distinct key**
(`web/prune.rs:91-105` pushes into `keys` with no lookup), while its own doc comment claims
"one allocation per distinct key rather than per use". Held until the part finishes. On the
N-form fixpoint that is ~2N² engine-heap allocations plus the Rust `Vec`. Web path only; I
could not drive it (`prune_output` is `pub(crate)`, so an external probe crate cannot call it).

**`held.every(p => p !== undefined)` skips holes**, so `worker-host.js`'s part-completeness
gate passes when only the LAST part arrived (`held.length` is still `of`). Verified in node.
ADR 0023 §3's "nothing is delivered unless every part succeeded" is guarded by a check that
cannot see the gap it names.

**How to drive the native split from outside the repo:** scratch crate, `burrow-ops` +
`burrow-engines` by path with `features=["native-engines"]`, `burrow_engines::OpenOptions`,
`burrow_engines::qpdf::Qpdf` (a unit struct, no `new()`), `burrow_types::SystemClock::new()`,
`LD_LIBRARY_PATH=engines/vendor/native-aarch64/lib`, and `cargo` is only on `~/.cargo/bin`.

See [[m1-split-pruning]] for what the policy itself gets wrong (items 1-4 there were fixed in
#54; the module now names each), [[m1-web-rotate-bridge]] for the hand-release comparison, and
[[m1-qpdf-exception-boundary]] for `trap_errors`' latching.
