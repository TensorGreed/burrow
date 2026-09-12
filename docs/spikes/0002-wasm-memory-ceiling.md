# Spike 0002 — a memory ceiling without relinking PDFium

Status: **complete**. Answers the second of the two questions
[ADR 0006](../adr/0006-wasm-linking-strategy.md)'s pre-M2 gate now carries, and recommends
removing it from that gate.

Spike code: branch `spike/wasm-memory-ceiling`, under `spikes/wasm-memory-ceiling/`. Not
merged. Nothing in `core/`, `bindings/` or `apps/web/` was changed to run any of this.

## The question

[Issue #25](https://github.com/TensorGreed/burrow/issues/25) established that
`max_memory_bytes` bounds nothing during an operation on the web, and that the obvious remedy —
supplying our own `WebAssembly.Memory` through `-sIMPORTED_MEMORY` — means **relinking PDFium
from source** instead of shipping the published artifact. That drags an engine-pin change, a
fresh licence audit and a rebuilt size budget into a gate that was about exception semantics.

So: **can the engine modules be given a real memory ceiling without relinking PDFium?**

A WebAssembly module's memory section declares initial and maximum pages. The prebuilt
`pdfium.wasm` declares a 2 GiB maximum at build time. If that field can be lowered in the
binary, unbounded growth becomes an allocation failure at a chosen point, on a path
[ADR 0009](../adr/0009-web-panic-contract-and-binding-boundary.md) already defines.

## Environment

| | |
|---|---|
| Machine | linux-aarch64, Node v24.14.0 |
| Engines | `pdfium.wasm` 5,315,922 B (prebuilt, chromium/8044); `qpdf.wasm` 1,199,200 B (built here) |
| Browsers | Chromium, Firefox, WebKit, via the existing `apps/web` Playwright suite |
| Corpus | `tests/conformance/expectations.json`, 22 cases × 2 operations |

Every browser run went through the **shipping path** — `tools/stage-web-engines.mjs` staging,
generated CSP, `blob:` worker, SRI-pinned fetch, the `worker-host` watchdog. No bypass entry
point, per [ADR 0016](../adr/0016-differential-conformance.md). The only thing that changed
between runs was the bytes of the engine modules, selected through the existing
`BURROW_ENGINE_ARCH` knob (`tools/stage-web-engines.mjs:179`).

## Finding 1 — the maximum is a 3-byte field, and lowering it changes nothing else

Both modules declare `flags=0x01, min, max` with max = 32768 pages (2048 MiB), encoded as the
3-byte LEB128 `80 80 02`:

| module | memory section | initial | maximum | max field |
|---|---|--:|--:|---|
| `pdfium.wasm` | offset `0x2276`, 7 bytes | 287 pages (17.94 MiB) | 32768 pages (2048 MiB) | 3 bytes at `0x227a` |
| `qpdf.wasm` | offset `0x1129`, 7 bytes | 256 pages (16 MiB) | 32768 pages (2048 MiB) | 3 bytes at `0x112d` |

A smaller number needs fewer bytes in its *minimal* LEB128 form, which would shrink the section
and shift everything after it — including the section's own size varint. **LEB128 permits
non-minimal encodings within the type's byte bound**, so the new value is written padded to
exactly the width the old one occupied. Measured, on both modules:

| target | pages | bytes changed | file size | compiles |
|---|--:|--:|---|:--:|
| 1024 MiB | 16384 | **1** | unchanged | yes |
| 512 MiB | 8192 | **2** | unchanged | yes |
| 256 MiB | 4096 | **2** | unchanged | yes |
| 128 MiB | 2048 | **2** | unchanged | yes |
| 64 MiB | 1024 | **2** | unchanged | yes |

`spikes/wasm-memory-ceiling/rewrite-max-memory.mjs`, ~200 lines, **no dependencies**.

**A targeted patch rather than a toolkit, deliberately.** `wasm-tools`, `binaryen` and `wabt`
all do this by decoding and re-encoding the module. Rejected on two grounds: a re-encode
rewrites the whole file, so the diff against the upstream artifact is every byte and *"we
changed the memory limit and nothing else"* stops being checkable; and adding a large
dependency to edit two bytes would need an [ADR 0008](../adr/0008-widened-licence-allowlist.md)
admission-test pass. With the patch, `cmp` is the audit.

That is asserted rather than claimed. `test-rewrite-max-memory.mjs` byte-compares the rewritten
artifact against the upstream original at every swept ceiling and **fails on any difference
outside the expected offsets** — a full comparison, not a length check. 14 adversarial cases:
refusing a non-wasm input, a module with no memory section, a memory with no declared maximum,
more than one memory, a raise, a maximum below the declared initial, and a planted difference
outside the field that the comparison must catch.

One thing that could not be tested, recorded because the attempt is informative: *"refuses a
value too wide for the existing field"* is **unreachable**. LEB128 width grows with magnitude,
so a value at or below the current maximum always fits the width that maximum already occupies.
The guard in `encodeVaruintPadded` is kept for a future direct caller and marked unreachable;
the test asserts the invariant instead.

## Finding 2 — the module's declared maximum is what binds, not the glue's

This is the finding the whole spike rests on, because Emscripten's JS glue carries **its own**
`maxHeapSize` from build-time `MAXIMUM_MEMORY`. If the glue's value bound first, patching the
module would achieve nothing and a bomb failing for some other reason would read as success.

Read out of both shipped glues:

```js
growMemory = size => { ... try { wasmMemory.grow(pages); updateMemoryViews(); return 1 } catch(e){} };
var _emscripten_resize_heap = requestedSize => {
  var maxHeapSize = getHeapMax();
  if (requestedSize > maxHeapSize) { return false }   // the GLUE's 2 GiB
  ...
```

So the sequence at the ceiling is: the request passes the glue's 2 GiB check → `wasmMemory.grow()`
throws `RangeError` because the **module's** declared maximum is lower → `growMemory` catches it
and returns undefined → `_emscripten_resize_heap` returns `false` → `malloc` fails.

Neither glue contains `abortOnCannotGrowMemory`, and both `_emscripten_resize_heap` failure
paths are `return false`, never `abort(`. That is a consequence of `-sALLOW_MEMORY_GROWTH=1`,
which flips Emscripten's default `ABORTING_MALLOC` to 0.

**The effective ceiling is the lower of the two, and after the patch that is ours.**

## Finding 3 — it loads and answers correctly in all three browsers

Full conformance corpus, 22 cases × 2 operations, through the shipping path, at a 1024 MiB
ceiling on both engines:

```
18 passed (40.4s)   [chromium, firefox, webkit]
  ✓ every corpus file produces the same typed outcome as the native path
  ✓ a deliberate engine failure discards the worker, and the next operation succeeds
  ✓ no ordinary outcome costs a worker, however hostile the file
  ✓ the watchdog kills a worker stuck inside a single engine call
  ✓ the circuit breaker stops a respawn loop, and only reset() restarts it
  ✓ the SAME file handle survives the worker that was reading it
```

Padded LEB128 is accepted by V8, SpiderMonkey **and** JSC. The SRI digest is computed over the
rewritten bytes by the normal staging step, and all three browsers fetched and validated them —
a mismatch would have failed the fetch outright.

**The watchdog is unaffected** (measurement 4): it still kills a worker stuck inside a single
engine call, and a start-up failure is still reported as `Internal` rather than `LimitExceeded`.

## Finding 4 — the transition tracks the patched value, which is what proves causation

A bomb that failed for an unrelated reason — a pre-scan refusal, qpdf's own 256 MiB flate
ceiling ([ADR 0013](../adr/0013-qpdf-c-api-and-prescan.md) §5), a watchdog kill — would look
exactly like success. So the ceiling was swept around each engine's **known peak**
([ADR 0015](../adr/0015-web-worker-lifecycle.md) §6), one engine at a time, with the other left
at 2048 MiB.

**PDFium**, whose `xref-bomb.pdf` peak is **1900.7 MiB**:

| ceiling | corpus | `xref-bomb` via `page_count` |
|--:|---|---|
| 2048 MiB | passes | `ok`, pdfium peak **1900.7 MiB** |
| 1984 MiB | passes | `ok`, pdfium peak 1900.7 MiB |
| **1920 MiB** | passes | `ok`, pdfium peak 1900.7 MiB |
| **1856 MiB** | passes | **`Internal`** — the bomb is now bounded |
| 1024 MiB | passes | `Internal` |
| 512 MiB | passes | `Internal` |
| 448 MiB | passes | `Internal` |
| 384 MiB | **FAILS** | — `objstm-bomb-default-limits` poisons the instance |
| 320 – 20 MiB | FAILS | same |

The transition sits between **1920 and 1856 MiB**, bracketing the measured 1900.7 MiB peak. It
is not a coincidence and it is not a coarse effect: the bomb succeeds while the ceiling is above
its peak and fails as soon as it is below.

**qpdf**, whose `xref-bomb.pdf` peak via `structure_check` is **513 MiB**:

| ceiling | corpus | note |
|--:|---|---|
| 2048 MiB | passes | |
| 1024 MiB | passes | |
| 640 MiB | passes | |
| **576 MiB** | passes | |
| **512 MiB** | 2 differences | `objstm-bomb` `structure_check` → `Malformed` |
| 256 – 32 MiB | 2 differences | same |

Transition between **576 and 512 MiB**, bracketing the measured 513 MiB peak. Two independent
confirmations, on two engines with peaks two orders of magnitude apart.

## Finding 5 — ordinary documents are unaffected almost all the way down

The corpus contains bombs, so "the corpus fails at 384 MiB" is not the same as "documents fail".
Separating them matters for choosing a value.

At a **20 MiB** PDFium ceiling — 2 MiB above the module's own 17.94 MiB initial, the lowest the
field can express:

```
  blank-1page.pdf    page_count       ok               pdfium 17.9 MiB
  pages-10.pdf       page_count       ok               pdfium 17.9 MiB
  pages-137.pdf      page_count       ok               pdfium 17.9 MiB
  truncated.pdf      page_count       Malformed        pdfium 17.9 MiB
  not-a-pdf.bin      page_count       Malformed        pdfium 17.9 MiB
  encrypted.pdf      page_count       PasswordRequired pdfium 17.9 MiB
  xref-bomb.pdf      page_count       Internal         pdfium    0 MiB
```

Every ordinary document still opens correctly, and every typed error is still the right one.
They simply never grow past the 17.9 MiB initial. So there are **two different thresholds**:

| | PDFium | qpdf |
|---|---|---|
| ordinary documents still correct | down to **20 MiB** | down to 32 MiB (lowest swept) |
| whole corpus unchanged, bombs included | **≥ 448 MiB** | **≥ 576 MiB** |
| the bomb stops reaching its peak | **< 1920 MiB** | **< 576 MiB** |

**A single ceiling for both engines must sit in [576, 1856] MiB** to leave every recorded corpus
outcome unchanged. 1024 MiB is comfortably inside it and halves the exposure; 640 MiB is the
tightest value with headroom above qpdf's 513 MiB peak.

## Finding 6 — which OOM route, and where each one lands

Two routes reach the bridge differently and both were characterised.

**Route (a), allocation refused.** Finding 2 establishes the mechanism: at the ceiling,
`malloc` returns 0 rather than aborting. When *burrow's own* `copyInto` is the caller
(`apps/web/src/worker/bridge.js:100`), that becomes `Error::Io`
(`core/burrow-engines/src/web/pdfium.rs:193`), which `burrow_wasm::is_fatal` already classifies
fatal — with a comment that predates this spike explaining why: a module at its ceiling is
poisoned for the worker's life.

**Route (b), the engine aborts mid-parse.** This is what the corpus actually exercises. PDFium's
internal allocator gets the null and takes its own out-of-memory path, which surfaces as a JS
exception out of the module and arrives as `Error::Internal` — the ADR 0009 "quiet abort" case.
Every measured `xref-bomb` failure took this route:

```
xref-bomb.pdf  page_count  Internal  pdfium 0 MiB
```

**Route (a) is not reachable through the current corpus**, and that is worth stating rather than
manufacturing: the largest fixture is 330 KB, so an input copy cannot fail at any ceiling the
module can express. Reaching it needs an input larger than (ceiling − initial), which at a
sensible ceiling means hundreds of megabytes — where `estimate::check_open_memory`'s
length-based pre-check refuses the file first. Route (a) is the mechanism; route (b) is the
behaviour.

Both end identically, and the recovery spec confirms it in all three browsers: instance fatal,
worker discarded, **next operation succeeds on a fresh worker**.

## Finding 7 — deterministic

Three repeat runs × three browsers = nine runs at a 1024 MiB ceiling. Byte-identical outcomes:

```
xref-bomb.pdf page_count      Internal    pdfium 0 MiB     qpdf 0 MiB
xref-bomb.pdf structure_check Unsupported pdfium 17.9 MiB  qpdf 513 MiB
```

No variation in outcome, error kind, or reported heap.

## Finding 8 — the cost is close to nothing

| | |
|---|---|
| **Artifact size** | **0 bytes** on both modules. Padded LEB128 keeps the length identical. |
| **Build time** | 0.04 s wall for the 5.3 MB module. |
| **Integrity pin** | Works unchanged. `stage-web-engines.mjs` computes the content hash and SRI over whatever it stages, so the digests already cover *our* bytes; all three browsers validated them. |
| **Upstream pin** | Unaffected. `engines/pins.toml`'s sha256 is over the **tarball**, and `engines/fetch.sh` verifies the tarball. A post-extraction rewrite is invisible to it — which is correct, and means both hashes coexist naturally: upstream's over what we downloaded, ours over what we ship. |
| **Component detector** | Fingerprint hit counts identical before and after (e.g. `icu_`: 6 and 6). The patch touches the memory section, which holds no symbols. **But** `tools/detect-engine-components.py:165` globs a literal `wasm/lib/*.wasm`, so it would not scan a differently-named prefix — an adoption detail, not a blocker, and an argument for rewriting in place rather than into a parallel prefix. |
| **Size budget** | Unaffected: `apps/web/size-budget.json` records raw and brotli, and raw is unchanged. Brotli of a 2-byte-different file may differ by a byte or two, far inside the headroom. |

## What was not measured, and why

- **qpdf via `-sMAXIMUM_MEMORY` instead of a patch.** Not rebuilt. `engines/build-wasm.sh:292`
  already passes `-sMAXIMUM_MEMORY=2GB` and the module declares exactly 32768 pages, so the flag
  and the patch write the same field. For qpdf — which we build — the flag is clearly the right
  mechanism; the patch exists for PDFium, which we do not build. Confirming they are
  byte-equivalent would cost an emcc rebuild and settle nothing the field values do not.
- **Raised `INITIAL_MEMORY` and open times.** Skipped as out of scope per the brief. The tool
  patches the maximum only; the initial is a separate 2-byte field that would fit the same
  padding trick, and `measure.spec.ts`'s respawn-cost instrument (72 ms median, chromium) is the
  natural place to measure it.
- **`measure.spec.ts` fails its own baseline assertion at low ceilings.** It asserts the
  post-corpus baseline has not dropped far below `MIN_CONVERGING_MEMORY_BYTES`, and after a bomb
  discards the worker the heap reads 0. A measurement artifact of that spec, not a spike result.

## The two things settled explicitly

### This is a per-worker ceiling, not a per-operation one

The module's maximum is fixed at instantiation and applies for the worker's life. **A
caller-supplied `max_memory_bytes` below it stays detect-only** — the 2026-09-12 amendment to
[ADR 0007](../adr/0007-limit-enforcement-per-platform.md) is unchanged by this spike. What the
patch adds is a hard floor under everything, not a per-operation limit.

A true per-operation ceiling still needs one of:

- **A fresh instance per operation.** ADR 0015 §6 measured respawn at 73–102 ms including
  recompiling all three modules. Not free, not prohibitive, and it would make the ceiling
  per-operation by construction. It also discards the memoised init promise every time.
- **An allocator hook counting per operation.** ADR 0007 already rejected a tracking allocator
  for Rust because the allocations are C++ ones it cannot see. Inside the engine module the
  arithmetic is different — `emscripten_resize_heap` is a single chokepoint — but it would mean
  a binding enforcing part of `Limits`, which ADR 0009 §2 forbids.
- **Imported memory** (`-sIMPORTED_MEMORY`), which is issue #25's direction and still means
  relinking PDFium. It would give a per-*instance* maximum, which is per-worker again unless
  combined with the first option.

### Whether mobile needs a different ceiling from desktop

ADR 0015 §7's numbers are the input: ordinary corpus work sits at ~18 MiB per engine, the
xref-bomb reaches 1.9 GiB, and on iOS a spike kills the whole tab rather than the worker.

Finding 5 says a single ceiling anywhere in [576, 1856] MiB leaves every corpus outcome
unchanged, and that ordinary documents are unaffected far below that. **So the evidence here
does not require two variants**: one ceiling can be both well above ordinary work and well below
2 GiB.

If a future measurement did require a lower mobile ceiling, the cost would be real: two build
variants of each engine, doubling the staged artifacts; a second content hash, SRI digest and
`connect-src` entry per variant, which the CSP's exact-URL policy makes awkward; a larger cache;
and variant selection at page load, before the engines are fetched, from a signal the page does
not currently collect. That is a significant amount of machinery to avoid choosing one number.

### What this spike cannot answer

It shows that allocation fails at a chosen point, deterministically, in three desktop browsers,
and that the failure lands where ADR 0009 requires. It **cannot** establish what ceiling an
iPhone tolerates. Desktop WebKit is not iOS, and ADR 0015 §7 already refuses to propose mobile
defaults from desktop readings — this spike does not change that and should not be read as
having done so.

**The mechanism question and the value question are separate. Only the mechanism question is
answered here.** Choosing the number still needs measurements on the hardware that matters.

## Recommendation

**Adopt the rewrite for PDFium, use the build flag for qpdf, and take reason 2 off the gate.**

The mechanism works, costs nothing measurable, lands on a failure path that already exists and
is already classified fatal, and leaves every recorded corpus outcome unchanged across a wide
window. It does not require relinking PDFium, so it does not drag an engine-pin change, a
licence re-audit or a rebuilt size budget into the pre-M2 decision.

Conditions I would attach:

1. **Rewrite in place, in `engines/build-wasm.sh`**, between the `cp` of the prebuilt artifact
   and the export assertions that follow it — so the existing checks run over the rewritten
   file, and `detect-engine-components.py`'s literal `wasm/lib/*.wasm` glob keeps working.
2. **Record both hashes.** Upstream's tarball sha256 stays in `engines/pins.toml`; add a
   recorded hash of our rewritten output so a rewrite that silently changed is caught. Both
   verified, neither inferred from the other.
3. **Ship the byte-comparison assertion**, not just the rewrite. The "only those bytes changed"
   property is the whole argument for a patch over a re-encode, and it is cheap to keep.
4. **Use the flag for qpdf.** We build it; patching a binary we produced would be indirection
   for its own sake.
5. **Pick the number separately.** 1024 MiB is defensible from the desktop evidence. It is not
   a mobile answer, and the ADR that adopts this should say so rather than letting a desktop
   curve become an implied iOS default.
6. **Update the three comments and one test that assert 2 GiB** — `apps/web/src/worker/bridge.js:87`,
   `bindings/burrow-wasm/src/bridge.rs:110` and its test at `:255`,
   `core/burrow-engines/src/web/pdfium.rs:167`. The `u32` pointer-coercion rationale at the
   first of those is load-bearing and gets *stronger* with a lower ceiling, not weaker.

## The gate question

> **Does this remove reason 2 from ADR 0006's pre-M2 gate?**

**Yes.**

Reason 2 was on the gate because the only known way to bound engine memory on the web ran
through `-sIMPORTED_MEMORY`, which means relinking PDFium — and relinking PDFium is an option-2
sized decision. This spike shows a real ceiling is reachable **without relinking anything**: two
bytes, no size change, no new dependency, no toolchain change, on a failure path ADR 0009
already defines.

What remains on the gate is what was always there: option 1 versus option 2 for redaction
safety — the shared-glue-globals hazard and trap-versus-unwind semantics — plus the open
question of whether a from-source PDFium `gn`/`ninja` build works on a `linux-aarch64` host.

Two qualifications, so this is not read as more than it is. The ceiling is **per worker, not per
operation**, so issue #25's underlying complaint — that `max_memory_bytes` is detect-only — is
*mitigated, not resolved*: a hard floor now exists under every operation, but the caller's
number still does not bound their own. And the **value** for that floor is not established by
desktop measurements. Both belong in the ADR that adopts this, not in the gate.
