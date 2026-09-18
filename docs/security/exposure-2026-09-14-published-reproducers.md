# Exposure assessment: CI published working reproducers for unpatched defects (2026-09-14 → 2026-09-18)

**This one is ours.** The sibling record in this directory,
`exposure-2026-09-14-qpdf-uaf.md`, is about a defect in someone else's library. This is about
how this project handled it: for five nightly runs across four days, CI published working
reproducers for those unpatched defects on a public repository, while the issue tracking them
said no reproducer existed anywhere.

Written as a file rather than left in issue comments because that is what this directory is for.
A comment on #62 is not in the tree, does not appear in a diff, and is not where the next person
looks.

## 1. What was published, and by which channel

**Two channels, and only the first was noticed at first.** That matters more than the incident
itself: the fix for the first channel was written, reviewed, and would have shipped while the
second was still open — with the summary text asserting the input was withheld.

### Channel 1 — the run artifact

`fuzz-nightly.yml` ended with `actions/upload-artifact` over `fuzz/artifacts/`, on
`if: failure()`. Each crash therefore uploaded the input that caused it as a named, downloadable
artifact. `gh run download <id> -n fuzz-artifacts-merge` was enough; no special access.

### Channel 2 — the job log, which the artifact deletion did not touch

The fuzz step's stdout and stderr are the public job log, and **the tooling prints the input into
it**:

- **cargo-fuzz** re-runs the target with `RUST_LIBFUZZER_DEBUG_PATH` set and prints
  ``Output of `std::fmt::Debug`:`` followed by the entire input as a decimal byte array. **There
  is no size limit.** Measured: all **16,208 bytes** of #119's reproducer were recovered from one
  line of the 2026-09-18 log, and decoded back to a valid PDF.
- **libFuzzer** prints `MS:`, a hex dump, an ASCII rendering and `Base64: <whole unit>` for any
  unit up to 256 bytes — smaller than most of this project's seed units.
- **libFuzzer's progress lines** carry `DE: "…"` dictionary entries, which are literal fragments
  of the input discovered by the mutator. Small, but input.

Retention for both channels is ~90 days by default.

## 2. Window and scope

| | |
|---|---|
| First published | **2026-09-14**, the same day the qpdf defects were found |
| Last published | **2026-09-18** |
| Span | **5 nightly runs across 4 calendar days** — both numbers appear in the record because they count different things; the runs are the unit that matters |
| Artifacts | 14, none expired |
| Logs | 5 run logs, all carrying channel 2 |
| Targets | `merge` ×5, `rotate` ×3, `reorder` ×3, `split` ×1, `compress` ×1 |
| Defects reachable from the published set | both #62 use-after-frees, and #119's stack overflow |

## 3. What was done

1. Both channels closed. The upload step is gone; the fuzz log is now passed through
   `tools/redact-fuzz-log.py` before anything is printed, in **both** `fuzz-nightly.yml` and
   `ci.yml`'s pull-request fuzz job — the second had the identical channel and no mitigation.
2. All 14 artifacts deleted, and all 5 run logs deleted. Both read back: the runs report zero
   live artifacts, and the logs endpoint returns 404.
3. `tools/check-no-published-reproducers.py` refuses any workflow or composite action that
   uploads an input directory, an ancestor of one, or a raw fuzz log.

**The redactor is an allowlist, not a list of things to strip.** A denylist publishes whatever it
has not been taught about, and the next cargo-fuzz release can add a new way of printing the
input. Only lines matching a rule that is argued to carry no input bytes are printed; everything
else is withheld. The cost is stated: a genuinely useful new line from a future libFuzzer is
dropped until someone adds a rule for it.

## 4. What cannot be undone, stated plainly

**Deletion is not recall.** Anyone who downloaded an artifact or a log inside the window still
has it. GitHub exposes no per-artifact or per-log download record, so there is no way to learn
whether anyone did — the honest statement is that the exposure existed and its use is unknown,
not that it was unused.

## 5. How it was found, and what that says

Not by the gate, the review of the gate, or the incident response. It was found by a security
review of **the fix for channel 1**, which read cargo-fuzz's and libFuzzer's pinned sources and
established that the log had been carrying the same payload the whole time.

Two things follow, and both are uncomfortable:

- **The first remediation was reported as complete when it was not.** The artifacts were deleted
  and the deletion read back, which is the correct procedure applied to the wrong boundary. Every
  statement made about it was true and the conclusion drawn from it was wrong.
- **The fix made the claim worse before it made it better.** The replacement step wrote "the
  input is NOT published" into the summary of a job whose log contained the input. An
  overclaiming comment is a bug in this repository; an overclaiming *security* comment is a
  reason someone stops checking.

The general form, which is the part worth keeping: **when a payload is withdrawn from one
channel, enumerate the channels.** "Where else does this data go?" was answerable at any point by
reading the log of the run whose artifact was being deleted.
