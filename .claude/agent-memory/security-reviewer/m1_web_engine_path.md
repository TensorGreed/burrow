---
name: m1-web-engine-path
description: Measured facts about the web engine path — the blob: worker bundle, what the CSP really covers, and (from 2026-09-15) the fact that PDFium is no longer in the web payload at all.
metadata:
  type: project
---

Facts about the web engine path that cost real digging, verified against the built
artifacts in `apps/web/public/engines/` and `engines/vendor/wasm/lib/`. Re-verified
2026-09-11 against commit `cf4ed23`.

**A worker from a same-origin script URL does not inherit the page's CSP.** Measured by the
maintainer during 4a-i: it takes its policy from that script's HTTP response headers, and a
static host sends none — the worker ran with no policy and a cross-origin `fetch` from
inside it reached the network, while every page-level CSP test passed. Only `blob:`,
`data:` and `about:` workers inherit. The design answer is: fetch the worker source with
`integrity`, `new Blob([source])`, `new Worker(blobUrl)`. This is the single most important
thing to preserve in any refactor of `apps/web/public/burrow-harness.js`.

**All worker code is one concatenated bundle** (BURROW_ENGINES const, prelude, bridge,
qpdf.js, pdfium.js, burrow_wasm.js, main.js) built by `tools/stage-web-engines.mjs`. Two
things I verified with acorn and `vm.runInThisContext`, so they do not need re-deriving:

  * **No top-level name collisions** across the six files (pdfium alone declares 655
    top-level names; bridge 9, main 5, prelude 4, qpdf and burrow_wasm 1 each). The only
    global-property assignment in any third-party file is a dead `window.prompt=`.
  * **`self.Module` survives pdfium.js.** `var Module` hoisting does not clobber it, and
    pdfium.js line 1 is `var Module=typeof Module!="undefined"?Module:{}` — the `typeof`
    guard is what actually preserves the prelude's config. That guard is in a third-party
    artifact re-fetched on every pin bump; a future Emscripten emitting an unconditional
    initialiser would silently drop `instantiateWasm`.
  * **The bundle runs in sloppy mode.** The `"use strict"` in prelude/bridge/main is inert,
    because `const BURROW_ENGINES = …` precedes it and breaks the directive prologue.

**`worker-src 'self' blob:` keeps `'self'` only so `e2e/worker-guard.spec.ts` can build the
bad worker.** Nothing in production creates a worker from a URL. ADR 0014 calls this "the
one widening this design needs, and it is narrow"; only `blob:` is needed.

**`INHERITS_PAGE_CSP` checks inheritance, not that a policy exists.** A blob worker created
by a document with no CSP passes the guard. The control that makes this safe today is
`src/production-build.test.ts` asserting the `<meta>` on every shipped HTML page.

**The prebuilt `pdfium.js` is NOT built with burrow's flags.** `engines/build-wasm.sh`
builds qpdf `-sENVIRONMENT=web,worker -sFILESYSTEM=0`; PDFium is unpacked from
`pdfium-wasm.tgz` as-is and still contains `ENVIRONMENT_IS_NODE`, `require("fs")`, a
synchronous `XMLHttpRequest` path and four `fetch(…, {credentials:"same-origin"})` calls.
None run (instantiateWasm pre-empts them, a worker is not Node) — but that is established
by the CSP and by tests, not by a build flag. The prelude comments now say so honestly.

**Now integrity-pinned: everything.** One digest over the whole bundle plus SRI on the
three `.wasm`. The earlier "only the .wasm is pinned, the 160 KB of glue is not" finding is
closed.

**The qpdf logger leak is closed** by `WebQpdf::install` behind an `Arc<OnceLock>` held in
a `thread_local!` engine — one logger per worker. `qpdflogger_cleanup` is still undeclared,
so `FakeHeap::assert_empty` remains structurally blind to that one allocation.

Related: [[m1-qpdf-exception-boundary]], [[m1-limits-real-strength]],
[[m1-engine-supply-chain]], [[m1-prescan-key-scan-bypass]].

## 2026-09-15 — spike 0004: PDFium is OUT of the web payload

Branch `m1-pdfium-off-the-web`. Re-verified against the working tree, not the diff.

**Three paragraphs above are now history, not current state.** The bundle is
`BURROW_ENGINES + prelude + bridge + qpdf.js + burrow_wasm.js + main.js` — **no
pdfium.js**, so the `self.Module` hoisting analysis, the `typeof Module` guard and the
load-order argument no longer describe anything shipped. `prelude.js` no longer assigns
`self.Module` at all, deliberately: a spare global of that name is configuration bait for
the next Emscripten glue added to the bundle. Two `.wasm` (qpdf 1,494,731 + burrow_wasm),
not three.

**`page_count` now runs on qpdf** (`StructureEngine::check`, `attempt_recovery = false`)
instead of PDFium. Verified by parsing the import section of
`bindings/burrow-wasm/pkg/burrow_wasm_bg.wasm`: **48 imports, zero `pdfium`**, every
`__burrow_*` base name defined in `bridge.js`. So no declared wasm-bindgen import can fail
to resolve at instantiation. Error text still comes from `codes::qpdf::map_code`, which is
fixed strings per code — no byte offsets, no object numbers.

**The CSP and the SRI pins are derived, not listed.** `policyFor()` builds `connect-src`
from the `staged` map that `ENGINE_FILES` produces, so dropping an entry drops its URL and
its pin together; there is no second place to forget. Confirmed on a real build.

**`tools/check-no-pdfium-on-the-web.sh` only tells the truth about a PRODUCTION build.**
`astro.config.mjs`'s `harnessGating()` deletes `dist/host/` unless `BURROW_HARNESS=1`, so a
harness build leaves `dist/host/harness-driver.js` there — and that file has the string
`__burrow_pdfium_copy_in` in a comment. Measured both ways on 2026-09-15: production
`pnpm build` → the checker and its 10-case self-test pass; the e2e/harness build → the
checker refuses and the self-test aborts at its baseline. CI is safe (its `Build` step sets
no flag); a developer who last ran `pnpm e2e` is not.
