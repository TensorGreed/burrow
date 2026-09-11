# fuzz

`cargo-fuzz` targets. One per parser entry point — that is a requirement, not a goal:
every input is treated as hostile.

Its own workspace (excluded from the root `Cargo.toml`) because cargo-fuzz requires that
and needs a nightly toolchain.

```bash
rustup toolchain install nightly
cargo install --locked cargo-fuzz
cargo +nightly fuzz list
```

## Running

From `fuzz/`:

```bash
LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
  cargo +nightly fuzz run document_open -- \
    -max_total_time=60 -timeout=10 -rss_limit_mb=2048
```

Requires the engines: `engines/fetch.sh && engines/build-native.sh`.

**`-timeout` and `-rss_limit_mb` are not optional.** `Limits::max_duration_ms` is
checkpoint-based ([ADR 0007](../docs/adr/0007-limit-enforcement-per-platform.md)): it is
only observed between engine calls, so a single `FPDF_*` call that runs forever is not
stopped by it and would just hang the fuzzer. libFuzzer's own `-timeout` is what catches
that — and a hang it finds is a bug in where our checkpoints are, not only in the input.

**`LD_LIBRARY_PATH` is not optional either.** `cargo fuzz run` executes the built binary
directly rather than through cargo, so it does not get the `<target>/<profile>/deps`
entry cargo adds for `cargo test` — which is how every other binary in the workspace
finds the pinned `libpdfium.so`.

**`detect_leaks=0`** because `FPDF_DestroyLibrary` is deliberately never called (tearing
PDFium down while anything could still be inside it is unsound), so its statics are live
at exit by design. That is a decision, not a leak.

## What our fuzzing covers, and what it does not

PDFium is a **prebuilt** binary: no sanitizer instrumentation, no coverage feedback
([ADR 0004](../docs/adr/0004-native-engines.md)). libFuzzer is driving a black box
through it, so `document_open` explores **our** wrapper, **our** error mapping and
**our** `Limits` enforcement — not PDFium's parser. Crashes *inside* PDFium would still
be detected as crashes, but coverage-guided exploration of its own code does not happen.

PDFium's internals are fuzzed upstream by OSS-Fuzz, continuously and with instrumentation
we are not going to match. **Claiming our fuzzing covers PDFium would be false.** If we
ever build PDFium from source ([ADR 0006](../docs/adr/0006-wasm-linking-strategy.md)'s
pre-M2 gate), instrumenting it becomes possible and this changes.

qpdf is different: we build it from source, and `engines/build-native.sh` produces a
second archive at `lib/fuzz/libqpdf.a` instrumented with
`-fsanitize=fuzzer-no-link,address`. See below.

## ASan interop with the instrumented qpdf archive — **resolved, it works**

Carried over from M1 PR 1 and settled during PR 2. The question was whether the clang 18
ASan runtime the instrumented qpdf archive expects is compatible with the Rust ASan
runtime cargo-fuzz links.

Measured on `linux-aarch64`, 2026-09-10:

| Check | Result |
|---|---|
| Host compiler | Ubuntu clang 18.1.3 |
| ASan interface version in `lib/fuzz/libqpdf.a` | `__asan_version_mismatch_check_v8` |
| ASan interface version in `librustc-nightly_rt.asan.a` | `__asan_version_mismatch_check_v8` |
| Links | **Yes** |
| Runs | **Yes**, and libFuzzer discovers new edges *inside* qpdf (`NEW_FUNC`) |

The interface versions match, so the usual failure — a duplicate `__asan_init` or an
"ASan runtime does not come first in initial library list" abort — does not occur.

To link the instrumented archive instead of the plain one, put its directory ahead on the
library search path:

```bash
RUSTFLAGS="-L native=$PWD/../engines/vendor/native-$(uname -m)/lib/fuzz" \
  cargo +nightly fuzz build <target>
```

The binary is visibly different — about 29 MB against 20 MB, and coverage counts rise —
so it is easy to confirm which archive was used.

**Nothing in this repository does that yet**, because no fuzz target calls qpdf. The
first one is M1 PR 3's, and it should use the override above. This section exists so PR 3
does not have to re-derive the answer.

Two caveats, since the measurement is narrower than the claim:

- Measured on aarch64 only. CI links engines on x86-64, where the clang version may
  differ. Re-check the two interface versions there before relying on it.
- The probe called one trivial qpdf function. It proves the runtimes coexist; it does not
  prove a heavy qpdf workload is clean under ASan. That is what PR 3's target is for.

## Findings

A target must never panic, abort, hang, or escape its `Limits` — regardless of input. A
crash is a bug even if the input is nonsense: nonsense input is the case that matters.

Findings go in `fuzz/artifacts/` (gitignored). When a crash is found: minimise it, add the
minimised input to the corpus, fix the bug, and add a unit test asserting the typed error
it should now produce.

## Targets

| Target | Entry point | Added |
|---|---|---|
| `document_open` | `DocumentEngine::open` + `page_count`, over PDFium | M1 PR 2 |
