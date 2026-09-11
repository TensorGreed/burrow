# 0013. qpdf through its C API, on the caller's thread, behind a Rust pre-scan

Date: 2026-09-10

## Status

Accepted. Amends [0004](0004-native-engines.md) — its repair justification, withdrawn by
[spike 0001](../spikes/0001-wasm-engines.md), is **re-established** with the corpus case
it asked for. See *The repair question, answered* below.

## Context

M1 PR 1 linked qpdf; nothing used it. This decision covers how it is called, and it had to
settle three things that looked much harder before the headers were read than after.

**C++ exceptions.** qpdf signals failure by throwing, and an exception unwinding into Rust
is undefined behaviour. The obvious plan was a C++ shim whose every exported function wraps
its body in `catch (...)`.

**Logging.** qpdf's default logger writes warnings to the process's stderr, quoting object
numbers and byte offsets — *including for files that parse successfully*
([spike 0001](../spikes/0001-wasm-engines.md), Finding 6). On the web that reaches the
devtools console. That is a non-negotiable #1 problem.

**The declared-size bomb.** [ADR 0011](0011-pdfium-engine-thread.md) records that M1 PR 2's
memory check is a function of the input's *length*, so a 330 KB file declaring twenty
million cross-reference entries drives PDFium to 1.2 GB. PR 2 added a measured check after
the load; the allocation still happens, and on a constrained device the engine can `abort()`
first. The fix has to run **before** anything parses the file, and it was deferred to here.

## Decision

### 1. The C API only — but **not** because it catches everything

The plan was a C++ shim wrapping every entry point in `catch (...)`. It is not needed, but
the reason is narrower than it first appeared, and getting that wrong cost a process abort
during review.

`qpdf-c.h:113-115` reads like a blanket guarantee:

> "If you encounter a situation where an exception from the C++ code is not properly
> converted to an error as described above, it is a bug in qpdf, which should be reported."

**It does not hold for every function.** Reading `libqpdf/qpdf-c.cc`, the catching is done
by one helper, `trap_errors` (`qpdf-c.cc:69`), and only functions routed through it are
covered. `qpdf_is_linearized` (`qpdf-c.cc:382`) is a bare
`return qpdf->qpdf->isLinearized()`. That calls `QIntC::to_int` on an object number, which
throws `std::range_error` above `INT_MAX` — so an ordinary 356-byte PDF containing
`9999999999 0 obj` kills the process:

```text
fatal runtime error: Rust cannot catch foreign exceptions, aborting
```

Measured end to end through `Qpdf::check`. The file parses, its page count is right, and
then the process dies. `catch_unwind` is no help: a foreign exception is not a Rust panic.
On the web that is the worker; on mobile, the app.

**So the rule is: call only functions verified to route through `trap_errors`**, verified by
reading `qpdf-c.cc` function by function rather than by reading the header's prose.

| function | trapped? | |
|---|---|---|
| `qpdf_read_memory` | yes | `qpdf-c.cc:297` |
| `qpdf_get_num_pages` | yes | `qpdf-c.cc:1801` |
| `qpdf_is_encrypted` | **no** | `qpdf-c.cc:389` |
| `qpdf_is_linearized` | **no** | `qpdf-c.cc:382` |

The consequence is visible in the API: **`StructureReport` reports only `pages`.**
`encrypted` and `linearized` were there and were removed, because the only safe way to
obtain them is the C++ shim this section says is unnecessary — and adding one to recover
two fields nothing yet consumes is the wrong trade at this point. Encryption remains
detectable through the error path, which is the case M1 actually needs:
`qpdf_e_password` means encrypted and not openable with what was supplied.

Everything else declared is non-parsing — `qpdf_init`, `qpdf_cleanup`, the setters, the
error accessors, the logger, the globals — assigning fields, flipping flags, or reading a
stored value. Those can fail only by allocation, which on Rust 1.81+ unwinding out of an
`extern "C"` frame is a defined abort rather than undefined behaviour. One caveat recorded
in `ffi.rs`: the four `qpdflogger-c.h` functions live in a file with no try/catch at all,
so they are in that accepted-abort-on-OOM category rather than the trapped one.

`core/burrow-engines/src/qpdf/tests.rs` drives an error through every entry point this
crate calls, and `testsupport/minimal_pdf.rs::pdf_with_object_number_above_int_max`
generates the file that used to abort — now in the damaged corpus, so reintroducing an
untrapped call aborts the suite rather than passing it.

**The lesson generalises past qpdf.** A header's prose described a guarantee its
implementation provides per-function. The reviewer read the `.cc`; the author read the
`.h`. For any FFI boundary where an exception could cross, the source is the contract.

### 2. Logging goes to `qpdf_log_dest_discard`, by name, from C

`qpdflogger-c.h` is a complete C binding for qpdf's logger, and `qpdf_log_dest_discard`
(`qpdflogger-c.h:62`) *is* the `Pl_Discard` pipeline. Item 7's "discarding `QPDFLogger`"
needs no C++ and, critically, no callback — so no Rust code ever runs on a C++ stack.

Three calls per document — `qpdf_silence_errors` and `qpdf_set_suppress_warnings(true)`
unconditionally, and `qpdf_set_logger` whenever the logger was created (it is skipped only
if `qpdflogger_create` returned null, which would mean qpdf could not allocate).
`qpdf_silence_errors` is the one most easily missed and matters most: without it, functions
that do not return an error code print to the host process's stderr.

**Measured, on this build, with the suppression removed:**

```
WARNING: input: file is damaged
WARNING: input (offset 4242): xref not found
WARNING: input, object 3 0 at offset 131: kid 0 (from 0) Resources is missing or invalid
```

Byte offsets and object numbers. `tests/secret_leak.rs` fails when the suppression is
removed, which is how that test earns the right to claim the suppression is load-bearing.

**One door the per-document logger does not cover.** `qpdf_cleanup` (`qpdf-c.cc:109-120`)
checks whether an error was left unretrieved and, if so, writes
`WARNING: application did not handle error: <text>` to `QPDFLogger::defaultLogger()` — not
the document's logger, and not gated on `qpdf_silence_errors`. That text carries byte
offsets. Every current path retrieves the error first, so it is unreachable today; because
that is a property of the control flow rather than of the type, `Document::drop` now drains
the error slot before cleaning up.

### 3. Errors are mapped by code, and the message functions are not even declared

`qpdf_error_code_e` has ten values whose numbering upstream guarantees across major
releases (`Constants.h:34-69`), and `qpdf_e_password` is a machine-readable "encrypted, and
the password did not work". So there is no temptation to match on prose.

`ffi.rs` deliberately does **not** declare `qpdf_get_error_full_text`,
`qpdf_get_error_message_detail`, `qpdf_get_error_filename` or
`qpdf_get_error_file_position`. A function that cannot be called cannot leak, and the
strongest available form of "never forward qpdf's messages" is to have no way to obtain
them.

**`QPDF_ERROR_CODE` is a bitmask, not an enum** (`qpdf-c.h:134-139`). `QPDF_WARNINGS` and
`QPDF_ERRORS` are separate bits, so the natural `result != QPDF_SUCCESS` reports a file
that parsed perfectly but emitted a warning as a failure. The correct test is
`result & QPDF_ERRORS`. This is the same class of trap as PDFium's `-0` sentinel and gets
the same treatment: one helper, one named test.

A related correction: `qpdf_e_system` is documented as "I/O error, memory error, etc." and
initially mapped to `Error::Io`. But this crate only ever calls `qpdf_read_memory`, on a
buffer it supplied — there is no file, no descriptor, no disk. Measured: a nine-byte input
of `%PDF-1.7\n` produces exactly that code. It maps to `Malformed`, which is what it means
here.

### 4. qpdf runs on the caller's thread, not PDFium's engine thread

`qpdf-c.h:43-45` permits it: distinct `qpdf_data` objects may be used from distinct
threads, and only sharing *one* between threads is forbidden. Every `qpdf_data` this crate
creates is born, used and destroyed inside a single function call and never escapes the
stack frame, so the rule is enforced by the type system rather than by discipline.

Putting it on PDFium's engine thread would be **worse**, not merely unnecessary. ADR 0011
records that one slow document already delays every other caller by up to 716 ms; adding
qpdf's work to that queue deepens exactly that coupling, and structural checks are meant to
be the cheap part that runs in parallel.

What *is* process-global — the resource limits and the discarding logger — is set once
behind a `OnceLock` before any `qpdf_data` exists.

### 5. qpdf's global limits are set, because they default to unlimited

qpdf 12.3 added `qpdf_global_set_uint32`, and **every decompression memory limit it offers
defaults to 0, meaning unlimited** (`global.hh:372-545`). So does the warning cap. A stock
qpdf inflates a decompression bomb until the allocator gives up, and an allocator giving up
inside C++ is an `abort()` — not a panic, not catchable, fatal to the process.

| Parameter | Set to | Default |
|---|---|---|
| `flate`/`dct`/`png`/`run_length`/`tiff` `_max_memory` | 256 MiB each | **unlimited** |
| `parser_max_nesting` | 64 | (set) |
| `doc_max_warnings` | 256 | **unlimited** |

`qpdf_p_limit_errors` is **not** set: it is read-only, and `global.cc`'s setter has no case
for it, so the call always returns `qpdf_r_bad_parameter`. It was briefly in the list with a
comment claiming it quieted qpdf's error path, which it never did.

**These are constants, not derived from `Limits`, and that asymmetry is deliberate.**
`Limits` is per-operation; these parameters are process-global and take no `qpdf_data`.
There is no way to give one caller a 16 MiB ceiling and another 1 GiB in the same process.
So they do different jobs: the caller's `Limits` are enforced in Rust per operation, and
these are a fixed floor under everything. A caller who sets a *tighter* ceiling still gets
it; one who sets a looser one does not get to raise these.

### 6. The pre-scan is bounded Rust, and does not use qpdf

The structural pre-scan runs before **either** engine sees the file. It reads declared
numbers only — `startxref`, the cross-reference `/Size`, `/W`, `/Index`, `/Prev` chain, and
every `/Length` — and never decompresses, resolves a reference, or recurses.

**It is not driven by qpdf, and that was the significant choice.** Using qpdf would mean
running a full C++ parser on completely ungated hostile input: the pre-scan would *become*
the new attack surface, bounded only by whatever limits the engine happens to offer. A
pre-scan that can itself be blown up has not removed the bomb, only moved it.

So it is pure Rust, `#![forbid(unsafe_code)]`, with O(1) allocation regardless of input, a
fixed cap on every loop, and no recursion at all — which makes the deeply-nested-object
bomb inexpressible against it rather than merely survivable.

**The load-bearing constant is not `sum(/W)`.** `/W` is how many bytes the *file* spends
encoding an entry, and a cross-reference stream compresses, so twenty million entries
occupy 330 KB on disk. What matters is what an engine allocates once it has them. Measured:
the bomb drives PDFium to 1,250,388 KB for 20,000,000 entries — almost exactly **64 bytes
per entry**. An earlier version of this check compared `entries × sum(W)` (340 MB) against
the ceiling and let the bomb straight through, because 340 MB fits comfortably inside the
1 GiB default.

**Reading the declaration is harder than it sounds, and the first version had three
bypasses.** Security review found all of them, each a one-line edit to the committed bomb,
each restoring the full 2.5 GB behaviour:

| evasion | why it worked |
|---|---|
| insert `/Sizes 1 ` before `/Size` | a literal first-match search read the decoy and gave up |
| pad the dictionary past 4 KiB | `/Size` fell outside the window |
| append 4 KB after `%%EOF` | `startxref` fell outside the tail window |

They share one shape, and it is the one worth remembering: each made the scan read
*nothing*, and a scan that found no cross-reference treated **"nothing declared" as
"nothing to declare"**. Widening windows does not fix that, because a file can always pad
further. So the fix is in three parts: keys must be followed by a PDF delimiter, every
occurrence is examined and the largest wins, and — the part that actually closes the class —
when the directed walk finds nothing, a whole-buffer scan for the largest `/Size` runs
instead. That backstop is deliberately blunt and deliberately last: on any file where the
directed walk works, it never runs.

Its honest limit: it sees declarations, not truth. A file whose declarations are modest and
whose content is hostile passes it, and PR 2's measured post-open check is still what
catches that. They are layers, not alternatives.

## The repair question, answered

[ADR 0004](0004-native-engines.md) listed "repair" among qpdf's jobs. Spike 0001 Finding 4
withdrew that justification: PDFium silently reconstructed a *fully corrupted* xref table
and returned the correct page count. M1 item 6 therefore said the claim needed a corpus
case PDFium actually fails.

**Two were found**, measured on this build with qpdf's recovery enabled:

| file | PDFium | qpdf |
|---|---|---|
| a valid 3-page file truncated to 290 bytes | `Malformed` | **1 page** |
| the same file with its trailer removed (526 bytes) | `Malformed` | **3 pages** |

So the repair justification stands again, and is now pinned by
`tests/structure.rs::qpdf_recovers_two_files_pdfium_refuses` rather than asserted in prose.
That test fails if PDFium ever learns to read these — which is the right outcome, because
the claim would then need re-examining rather than quietly continuing to be cited.

Recovery is **off by default** in `CheckOptions`, because a structural check that silently
repairs the structure it is checking answers "could this be made to work?" when the caller
asked "does this work?". It is also where a damaged file makes qpdf expensive, and where it
becomes chatty.

## Consequences

No C++ in the repository, and none planned. The FFI surface is nineteen hand-written
declarations whose signatures a reviewer can check against cited header lines.

The pre-scan is a small amount of PDF lexing we now own and must keep bounded. It is
explicitly **not** a parser and must not grow into one: a question that needs the object
graph belongs in the qpdf module, behind qpdf's limits. That boundary will be under
pressure the first time something almost fits.

qpdf's global limits being process-wide means per-operation memory tuning for qpdf is not
achievable in-process, and a future caller who wants it will have to be told no.

**One upstream hazard our threading decision makes reachable.** qpdf 12.3's global limit
counters are plain `uint32_t` statics incremented from the parse path
(`global_private.hh:152`), not atomics. `qpdf-c.h:43-45`, which §4 relies on, guarantees
only that distinct `qpdf_data` objects are safe — it predates that global state. Two
threads inside `check` on limit-tripping files therefore race on a counter. Out of scope
per `SECURITY.md` (it is upstream's), and burrow reads none of those counters, but it is
recorded here because running on the caller's thread is what makes it reachable at all.

`fuzz/libqpdf.a` is now covered by `lib/BUILD_MANIFEST.sha256`, which it was not — an
archive linked into fuzz binaries that nothing had ever checksummed.

### Still not solved, and now written down

**A hang inside an engine cannot be interrupted from inside this process.** `Limits`'
time budget is checkpoint-based ([ADR 0007](0007-limit-enforcement-per-platform.md)) and
qpdf offers no timeout, no cancellation and no abort hook — the progress reporter returns
`void` and covers writes only. A single engine call that runs forever is not stopped by
anything we have.

The answers differ per platform and none of them is in M1:

- **Web**: recovery means terminating the worker, which M1 PR 4 builds anyway for
  [ADR 0009](0009-web-panic-contract-and-binding-boundary.md)'s panic contract. The same
  mechanism covers a hang.
- **Android**: a separate service process is available, and is the obvious route.
- **iOS**: **no subprocesses are permitted at all.** So process isolation cannot be the
  uniform answer, and M3/M4 must decide per platform rather than inheriting one design.

This is the same constraint that makes an engine out-of-memory unsurvivable (ADR 0011), and
it should be settled once for both.

## Alternatives considered

**A C++ shim with `catch (...)` on every entry point.** The obvious design, and what the
brief for this PR assumed. Rejected once the headers were read: it would duplicate a
guarantee upstream already provides and treats as a bug when it fails, while adding C++ to
a repository that currently has none.

**`qpdfjob-c.h`, the JSON job interface.** It wraps the CLI's logic and can emit an object
map as JSON. Rejected: every input is a **filename** — there is no memory entry point — so
it would force temp files for data we already hold, which is worse for both privacy and
speed. Its errors are also CLI exit codes, which would throw away `qpdf_e_password` and the
whole structured error surface.

**Shelling out to the `qpdf` binary.** Not available: no CLI is built anywhere in the
vendor tree, and building one would mean a new binary to ship, sandbox and audit for
strictly less than the library already gives us in-process.

**Driving the pre-scan with qpdf.** Discussed above. Rejected because it makes the
pre-scan the new attack surface.

**Running qpdf on PDFium's engine thread.** Simpler to reason about — one place all engine
work happens. Rejected because qpdf does not require it and it would deepen ADR 0011's
head-of-line blocking for work that is supposed to be the cheap parallel part.
