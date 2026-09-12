# Spike 0003 — a from-source PDFium build, and what is left of option 1's weakness

Status: **complete**. Investigates the two unanswered inputs to
[ADR 0006](../adr/0006-wasm-linking-strategy.md)'s pre-M2 gate. **Recommends, does not decide.**

Spike work: branch `spike/pdfium-source-build`. No build was completed and none was expected;
the brief was time-boxed and asked for the answer and its cost.

## The question

The gate is back to one question after [spike 0002](0002-wasm-memory-ceiling.md) removed the
memory-ceiling reason: **stay on option 1** — the published PDFium WASM artifact, engines as
separate Emscripten modules bridged through JS — **or move to option 2**, PDFium built from
source under one linking model.

Two things were load-bearing and neither was answered.

## Environment

| | |
|---|---|
| Machine | linux-aarch64, Ubuntu 24.04.4, kernel 6.17, 20 cores, 121 GiB RAM |
| Disk | 2.7 TB free |
| Privileges | unprivileged (uid 1000), no root |
| Present | `ninja`, `cmake`, `clang`, `git 2.43`, `python3 3.12` |
| Absent | `gn` |

## Finding 1 — depot_tools works on aarch64. That was not the blocker.

The expected answer was "Chromium tooling is x86-64 only". It is wrong, and I nearly recorded
it anyway: running `ensure_bootstrap` from the wrong directory produced

```
Platform linux-arm64 is not supported by the CIPD client bootstrap:
there's no pinned SHA256 hash for it in the *.digests file.
```

That message is real and it is **not what it looks like**. `cipd_client_version.digests` lists
**37 platforms including `linux-arm64`**; the failure was a relative path resolving against the
wrong working directory. Run correctly, everything resolves:

| | |
|---|---|
| CIPD client | `infra/tools/cipd/linux-arm64`, runs, reports its instance ID |
| depot_tools Python | bootstraps, `Python 3.11.8` |
| `gn` | `gn/gn/linux-arm64` exists in CIPD, **built daily** (latest instance Sep 11 2026) |
| `cipd_manifest.txt` | declares `linux-arm64` a `$VerifiedPlatform` |

Recorded prominently because a false "aarch64 is unsupported" would have been an input to an
architectural decision, and it was one bad shell invocation away from being this report's
headline.

## Finding 2 — upstream PDFium has no WASM target at all, and that is the blocker

A shallow clone of `pdfium.googlesource.com/pdfium` is **67 MB and takes under two seconds**.
Searching the entire tree for Emscripten or WASM support:

```
$ grep -rliE 'emscripten|wasm' --include='*.gn' --include='*.gni' .
third_party/harfbuzz/BUILD.gn
```

**One file.** Its hits are `hb-wasm-api-blob.hh`, `hb-wasm-api-buffer.hh` — HarfBuzz's own
WASM *shaper* API source filenames, nothing to do with building PDFium for the web. `DEPS`
mentions Emscripten zero times. There is no `target_os = "emscripten"`, no wasm toolchain, no
wasm config.

So **"build PDFium from source for the web" does not mean "run `gn` with a wasm target."** It
means adopting the patch set that makes `target_os = "emscripten"` exist at all —
`pdfium-binaries`' patches to Chromium's `BUILDCONFIG.gn` and `//build/toolchain/wasm`, which
[spike 0001](0001-wasm-engines.md) recorded and this spike confirms against the current tree.

That reframes option 2. It is not "stop using someone else's binary and build it ourselves".
It is **"fork or reproduce a third-party packaging project's patch set against an upstream
that does not support the target"** — and then carry those patches across every PDFium bump,
forever, on a configuration upstream does not test.

The `gn`-on-aarch64 question that the gate has been holding open turns out not to matter much:
`gn` runs here, and the thing it would run on does not exist.

## Finding 3 — the shared-glue-globals hazard is closed for PDFium by construction, and for qpdf only by a memoisation nothing tests

Spike 0001's HIGH finding: loading `pdfium.js` twice in one scope rebinds every glue global
while the bridge still holds the first instance, so calls read and write the *other* instance's
linear memory. The sandbox holds; parses come back confidently wrong.

The two engines are in different positions today, and the difference has not been written down
before.

**PDFium: closed structurally.** `pdfium.js` is **not** modularised — its state lives in worker
globals and it begins instantiating as it is parsed. Two things follow:

- The worker is **one bundled file** (ADR 0014 §1a), constructed from a `Blob`. There is no
  second `importScripts` to make, because there is no `importScripts` at all.
- A worker scope evaluates that bundle exactly once. A *second worker* has an entirely separate
  global scope, so it shares nothing.

`apps/web/src/worker/prelude.js:22` states the property — "loaded exactly once by construction" — and two tests
would fail if it were undone: `apps/web/src/production-build.test.ts:68` asserts the worker ships as **one**
`.js` under `engines/` with the glue markers inside it, and `apps/web/e2e/integrity.spec.ts:70` carries a
size floor that fails if the glue moves back out of the bundle.

**qpdf: closed by the memoised promise, and nothing tests the memoisation.** `qpdf.js` **is**
`MODULARIZE`'d, so `createQpdfModule()` is an ordinary function that returns a fresh, fully
independent instance every time it is called. What prevents a second call is one line in
`apps/web/src/worker/main.js:77`:

```js
ready ??= init();
```

The comment above it is precise about why it is a promise and not a boolean. But:

> **No test would fail if `ready` reverted to a boolean flag set after the `await`.**

I looked: nothing in `guard.test.ts`, `worker-host.test.ts` or any e2e spec asserts that init
runs once, or that the memoised value is a promise. With a flag, two messages arriving before
init finished would each run `init()`, calling `createQpdfModule()` twice — and the bridge's
`qpdfModule` would be rebound to the second instance while an in-flight call still held the
first. **That is the original HIGH finding, reachable through qpdf rather than PDFium.**

Two things reduce the exposure without closing it: ADR 0015 §2a serialises operations
page-side, so two requests are not normally in flight; and the bundle's structure means PDFium
cannot be reloaded either way. Neither is a test, and the page-side queue is a different layer
from the worker-side invariant.

**So: option 1's shared-glue weakness is closed for PDFium by construction and for qpdf by an
untested line.** The honest statement for the gate is that the weakness is *narrow and
addressable with a test*, not that it is gone. The test is cheap — assert `ensureReady()`
returns the identical promise across two concurrent calls — and is worth writing regardless of
which way the gate goes.

## What option 2 would buy, against today's code

Restated against the current tree rather than the spike-0001 prototype:

| | worth |
|---|---|
| **`catch_unwind` works** on `wasm32-unknown-emscripten` | Real. ADR 0009 records that on `wasm32-unknown-unknown` a panic is an uncatchable trap and `burrow-ffi::guard` is inert on the web. Option 2 restores ADR 0002's rule on all three platforms. **This is the strongest argument and the one the gate was written for.** |
| **One linking model** | Real but smaller than it was. The cross-heap copy is one `copy_in` per operation; `measure.spec.ts` puts ordinary work at ~18 MiB per engine, so the copy is not a cost anyone has measured as a problem. |
| **`-sIMPORTED_MEMORY`** for a per-operation bound | **Weakened by spike 0002.** A per-*worker* ceiling is already reachable with a 2-byte patch and no relink. Imported memory would still be per-instance, i.e. per worker, unless combined with a fresh instance per operation — so it does not by itself deliver the per-operation bound issue #25 wants. |
| **Control over the component set** | Real, and cuts both ways — see below. |

## What it would cost

- **Every size budget re-measured.** `apps/web/size-budget.json` records raw and brotli per
  artifact with a 3% total headroom. A from-source build changes `pdfium.wasm` entirely.
- **A fresh licence audit.** `pdfium-binaries` derives each platform's `licences/` directory
  from *that platform's* `build.ninja`. A build with different `args.gn` has a different
  component set, so `engines/licenses.toml`, the committed texts under `engines/licences/` and
  the credits page roster all move together. ADR 0010's HarfBuzz and ICU findings came out of
  exactly this surface.
- **Reproducibility.** `engines/pins.toml` pins a tarball by sha256 and `fetch.sh` fails closed.
  A source build replaces that with "whatever `gclient sync` resolved", which is a weaker
  provenance story than the one we have — and issue #22 shows the Emscripten cache alone
  already made a build non-reproducible once.
- **The patch set, forever.** Finding 2's real cost. Patches to Chromium's `BUILDCONFIG.gn` and
  wasm toolchain, rebased across every bump, on a configuration upstream does not test.
- **Sync and build.** Spike 0001 recorded ~15–20 GB before anything compiles; not reproduced
  here, and the shallow PDFium tree alone is 67 MB before `DEPS` pulls Chromium's build system.

## What is unaffected either way

The qpdf `trap_errors` audit; `known_gap` governance; the differential conformance harness; the
worker lifecycle, watchdog, circuit breaker and recycling; the CSP and integrity pinning; every
check hardened in PRs #39–#42. None of these depends on how PDFium is linked.

## Can the decision be deferred again?

**Yes, and at low cost — but not indefinitely, and the reason is specific.**

What M2's redaction work actually needs from this decision:

1. **Confidence that a parse result came from the heap it was supposed to.** This is the
   shared-glue hazard, and Finding 3 says it is closed for PDFium by construction. A redaction
   pass that "inspects one heap and edits another" — ADR 0006's words — cannot happen through
   PDFium's glue in the current design.
2. **A recoverable failure when redaction verification fails.** That is ADR 0009's contract,
   which is implemented and tested in three browsers today.
3. **`catch_unwind` on the web.** This is the one M2 genuinely might want, because redaction is
   where a panic mid-operation is most consequential — and option 1 cannot provide it.

So the concrete forcing question is (3), and it is narrower than "option 1 vs option 2": does
M2 need a Rust panic during redaction to be *caught and typed*, or is "terminate the worker and
report `Internal`" — which is what happens today, tested — sufficient? If the latter,
the decision defers again at no cost. If the former, it does not.

## Recommendation

**Stay on option 1 for now. Do not schedule a from-source build.** Conditions:

1. **Write the memoisation test** (Finding 3) before M2 starts, whichever way the gate goes.
   `ensureReady()` must return the identical promise across concurrent calls. It is the one
   real gap this spike found, it is cheap, and it closes option 1's remaining shared-glue
   exposure.
2. **Re-frame the gate's open question.** It is no longer "does `gn` work on aarch64" — it does.
   It is "does M2 need `catch_unwind` on the web". Answer that and the gate answers itself.
3. **If option 2 is ever taken, budget it as a fork, not a build.** Finding 2 is the number
   that matters: upstream has no wasm target, so the ongoing cost is carrying a third party's
   patch set across every bump.
4. **Do not let spike 0002's ceiling drift into this decision.** It is reachable under option 1
   with a 2-byte patch, and it is per-worker under either option.

## The gate, restated

> **Option 1, or a from-source PDFium build, for redaction safety?**

Unchanged by this spike, but better informed:

- The aarch64 question is **answered**: the tooling works, and it is not the obstacle.
- The obstacle is that **upstream PDFium has no WASM target**, so option 2 is a patch-set fork.
- Option 1's shared-glue weakness is **closed for PDFium structurally** and closed for qpdf by
  one untested line — so the remaining difference between the options narrows to
  trap-versus-unwind.

**Nothing here is decided.** ADR 0006 is untouched, no adoption is proposed, and M2 has not
started.
