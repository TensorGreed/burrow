---
name: fuzz-report-steps
description: 2026-09-25 review of fuzz-classify/describe scripts, the classifier's report_of fix, the panic-location redactor rule and fuzz-reproduce.yml; the step summary is an unredacted channel
metadata:
  type: project
---

Branch ci/fuzz-nightly-tooling-failures (d5e11d9), reviewed 2026-09-25 before push.

Measured:
- **The step summary is a publication channel the allowlist redactor never sees.** The classifier's
  `VERDICT` and describe's verdict grep are unanchored, so an `ERROR: libFuzzer: ` substring inside a
  Rust panic message (printed BEFORE the real report; cmap's assert Debug-formats the input) is
  taken as the verdict and the text after it lands in GITHUB_STEP_SUMMARY. Synthetic log reproduced.
  Fix: anchor on `^==\d+== ?ERROR: (AddressSanitizer|libFuzzer):` with re.M in both.
- **fuzz-describe.sh dies with SIGPIPE (141) at the offset pipeline** (`sort | ... | head -1` under
  pipefail) once the log has ~1000 unique `target+0x` offsets -- and it greps the WHOLE log,
  NEW_FUNC coverage lines included (the classifier's own fixed defect, left in the sibling).
  700 passed 5/5, 1000 failed. The trap labels it tooling, but the job still goes red on KNOWN crashes.
- `grep -qxF -- "$TARGET"` treats a newline in the input as a second pattern: `rotate\n<anything>`
  validates, and the extra line lands in GITHUB_OUTPUT, then `${{ steps.fuzz.outputs.target }}`
  in a later `run:`. Needs write access to dispatch, so Low.
- libFuzzer -fork (0.4.13 FuzzerFork.cpp): parent exits with the LAST popped job's code; an ignored
  timeout (70) / OOM (71) right before time-up gives non-zero with no report -> "INFRASTRUCTURE".
- Self-test case 8c survives reverting BOTH report_of and the frame-line anchor (MAX_FRAMES 200 >
  46 offsets); only dies if MAX_FRAMES is also 40. Redactor panic rule: only `$` has a near-miss;
  `^`, path class, thread-name class and id group all survive mutation.

**How to apply:** when a fuzz workflow step writes to the summary, check it against the redactor's
standard too; and when a fix lands in one of two sibling scripts, grep the other for the same shape.
Related: [[m1_ci_check_vacuity]].
