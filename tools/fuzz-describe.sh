#!/usr/bin/env bash
# The fuzz workflows' description step: what the crash was and where, never the input.
#
#   tools/fuzz-describe.sh <target> <log> [binary] [artifacts-dir]
#
# Called by `.github/workflows/fuzz-nightly.yml` and `fuzz-reproduce.yml`. Publishes a verdict, a
# frame name and an input hash; never a byte of input -- this repository is public and #62's
# defects are unpatched upstream.
#
# GITHUB_STEP_SUMMARY defaults to stderr outside Actions.
#
# Self-test: tools/test-fuzz-report-steps.sh.
set -euo pipefail  # -e as the workflow step ran it: a failing command ends the step
target="$1"
log="$2"
binary="${3:-fuzz/target/$(uname -m)-unknown-linux-gnu/release/$target}"
artifacts="${4:-fuzz/artifacts/$target}"
: "${GITHUB_STEP_SUMMARY:=/dev/stderr}"


# THIS STEP DYING IS A TOOLING FAILURE, AND SAYS SO. For six nights (2026-09-20..25) it
# died silently at the frame lookup below: an unsymbolised report has no ` in <symbol>`
# line, grep exited 1, and `bash -e` with `pipefail` ended the step before it wrote
# anything -- so its failure read exactly like the crash it was describing. Every grep
# that may match nothing is now guarded, and anything else that kills the step names
# itself as the tool, with the line, rather than joining the crash's red.
trap 'echo "::error title=DESCRIBE STEP FAILURE (tooling, not a crash)::died at line $LINENO" >&2; echo "- **DESCRIBE STEP FAILURE** at line $LINENO: the tool, not the crash. The Classify verdict above stands." >> "$GITHUB_STEP_SUMMARY"' ERR

# NO HEADING HERE. The Classify step above already wrote `### <target>`; two of them
# per target is a summary that reads as two crashes.
# DID THE TARGET EVEN RUN? A missing log, or one with no sanitiser or libFuzzer marker
# in it, means the step died before fuzzing -- a compile error in the target, a linker
# failure against the instrumented archive. That is not a crash and must not be
# reported as one.
# THE REPORT, SLICED FROM ITS OWN BANNER: `==N==ERROR:` (ASan) or `==N== ERROR:` (libFuzzer) at the
# start of a line. Everything before it -- coverage lines, and a Rust panic message that the CMap
# target fills with its whole input -- is not the report, and reading it was two defects found by
# security review (2026-09-25): an input carrying `ERROR: libFuzzer: ` became the published
# verdict, and a real-sized log's coverage lines killed this step (see `sed -n 1p` below).
BANNER='^==[0-9]+== ?ERROR: (AddressSanitizer|libFuzzer):'
report="$(mktemp)"
trap 'rm -f "$report"' EXIT
awk -v banner="$BANNER" 'found || $0 ~ banner { found = 1; print }' "$log" > "$report" 2>/dev/null || true

if [ ! -s "$log" ] || [ ! -s "$report" ]; then
  {
    echo "The fuzz step failed **without producing a sanitiser report**: this is a"
    echo "build or infrastructure failure, not a crash. Read the step log."
  } >> "$GITHUB_STEP_SUMMARY"
  echo "no sanitiser report; not describing this as a crash"
  exit 0
fi

{
  echo "A crash. The input is **not published**: this repository is public and #62's"
  echo "defects are unpatched upstream. Verdict and location only; reproduce locally"
  echo "against a seeded corpus."
  echo ""
} >> "$GITHUB_STEP_SUMMARY"

# THE SANITISER'S VERDICT, AS THE WHOLE LINE. A character class was wrong here twice:
# `[a-z- ]` read `z- ` as a range and grep exited 2 with "Invalid range end" (hidden by
# a `2>/dev/null`, so every crash reported as "timeout or OOM"), and `[a-z-]` then
# silently dropped the UPPERCASE verdicts -- `SEGV` and `DEADLYSIGNAL`, which is how a
# stack overflow surfaces when ASan's own detector does not engage. That is #119's
# class, reported as a timeout. Take the rest of the line and trim it instead of
# trying to spell the set of verdicts.
# `sed -n 1p`, NEVER `head -1`, IN EVERY PIPELINE HERE. `head` exits after one line, the stage
# before it dies of SIGPIPE (141), `pipefail` makes that the pipeline's status and `-e` ends the
# step -- measured by review at ~1,000 distinct offsets, which a real seeded log exceeds.
verdict="$({ grep -hoE "$BANNER.*" "$report" || true; } | sed -n 1p \
  | sed -E 's/^==[0-9]+== ?//' | cut -c1-120)"
echo "- verdict: \`${verdict:-unknown (marker present but unparsed; read the log)}\`" \
  >> "$GITHUB_STEP_SUMMARY"

# AND WHERE. Two shapes: a symbolised report (` in QPDF::...`), and raw offsets into
# the target binary, which is what a reader would otherwise download the input to
# resolve. Prefer the symbols when they are there. Either way this publishes a NAME,
# never a byte of input.
# `{ grep … || true; }`: THE LINE THAT KILLED THIS STEP. See the trap above.
# FRAME LINES OF THE REPORT ONLY (`    #3 0x… in Name`), as the classifier reads them.
frame="$({ grep -hoE '^[[:space:]]*#[0-9]+ 0x[0-9a-f]+ in [A-Za-z_][A-Za-z0-9_:]*' "$report" || true; } \
  | sed 's/.* in //' | sort | uniq -c | sort -rn | sed -n 1p | awk '{print $2}')"
if [ -n "${frame:-}" ]; then
  echo "- hottest frame: \`$frame\` (from a symbolised report)" >> "$GITHUB_STEP_SUMMARY"
else
  # `fuzz/target`, not `target`: this step runs at the repository root while the
  # previous one runs in `fuzz/`, and cargo-fuzz writes to the fuzz crate's own target
  # directory. The first version looked in the wrong place, so `addr2line` never ran
  # and the step reported "no offset in the report" -- a claim about the log -- when
  # the offset was there and the binary was not.
  offset="$({ grep -hoE "^[[:space:]]*#[0-9]+ 0x[0-9a-f]+ +\\([^ ]*$target\\+0x[0-9a-f]+\\)" "$report" || true; } \
    | sed -E 's/.*\+(0x[0-9a-f]+)\)$/\1/' | sort | uniq -c | sort -rn | sed -n 1p | awk '{print $2}')"
  if [ -z "${offset:-}" ]; then
    echo "- hottest frame: no offset in the report and no symbols either" \
      >> "$GITHUB_STEP_SUMMARY"
  elif [ ! -f "$binary" ]; then
    # LOUD, because it means this step is inert: the report had what it needed and
    # the binary to resolve it against was missing.
    echo "- hottest frame: offset \`$offset\`, but \`$binary\` is NOT on disk -- this" \
      "step could not symbolise and is not doing its job" >> "$GITHUB_STEP_SUMMARY"
  else
    resolved="$(addr2line -f -C -e "$binary" "$offset" 2>/dev/null | sed -n 1p)"
    echo "- hottest frame (\`$offset\`): \`${resolved:-unresolved}\`" \
      >> "$GITHUB_STEP_SUMMARY"
  fi
fi

# THE INPUT'S FINGERPRINT, so one night's crash can be told from another's without
# anyone holding the bytes.
found=0
for f in "$artifacts"/*; do
  [ -f "$f" ] || continue
  found=$((found + 1))
  {
    echo "- input sha256: \`$(sha256sum "$f" | cut -d' ' -f1)\`"
    echo "  - size: $(wc -c < "$f") bytes, NOT published"
  } >> "$GITHUB_STEP_SUMMARY"
done
if [ "$found" -eq 0 ]; then
  echo "- no artifact on disk: libFuzzer died without writing one (timeout or OOM)" \
    >> "$GITHUB_STEP_SUMMARY"
fi
echo "$found crash input(s) hashed, none published"
