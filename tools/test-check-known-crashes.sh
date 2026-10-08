#!/usr/bin/env bash
# Test that the known-crashes ledger reports what it owns and refuses what it does not.
#
# The case that justifies the whole design is case 4: the SAME frame under a different verdict is
# a different defect. #62's page-tree use-after-free and #119's stack overflow both go through
# `pushInheritedAttributesToPageInternal` -- measured, from two reproductions in this repository
# -- so a ledger keyed on the frame alone would have silenced #119 the moment #62 was entered,
# which is the exact failure this mechanism exists to prevent.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
work="$(mktemp -d)"
mutant="$here/check-known-crashes.MUTANT.py"
trap 'rm -rf "$work" "$mutant"' EXIT

EXPECTED_CASES=21
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

report() {
  printf '==1==ERROR: AddressSanitizer: %s on address 0x1\n    #0 0x1 in %s(QPDFObjectHandle)\n' \
    "$1" "$2" > "$work/report.log"
}

expect() {
  local name="$1" want="$2" needle="$3"
  local out status=0
  out="$(python3 "${4:-$here/check-known-crashes.py}" --ledger "$work/ledger.toml" \
    --log "$work/report.log" 2>&1)" || status=$?
  if [ "$status" -ne "$want" ]; then
    bad "$name (exit $status, wanted $want)"
    sed 's/^/         /' <<<"$out" >&2
  elif ! grep -qF "$needle" <<<"$out"; then
    bad "$name: right exit, wrong reason"
    echo "       expected: $needle" >&2
    sed 's/^/         /' <<<"$out" >&2
  else
    ok "$name"
  fi
}

# A LEDGER OF ITS OWN, for the same reason `PROBE_LEDGER` exists in the tool: the day #62 is
# fixed, `--check-issues` will correctly demand its entries be deleted -- and a suite asserting
# against those entries would go red in CI and block the very cleanup the mechanism requires.
# The fixture keeps the SHAPE that matters: two issues sharing a frame under different verdicts.
cat > "$work/ledger.toml" <<'LEDGER'
[[known]]
issue = 1
verdict = "heap-use-after-free"
frame = "Shared::Frame"
note = "a fixture, not a real defect"

[[known]]
issue = 2
verdict = "stack-overflow"
frame = "Shared::Frame"
note = "the same frame, a different verdict -- the case the pair exists for"

[[known]]
issue = 3
verdict = "heap-use-after-free"
frame = "Distinct::Frame"
note = "a fixture with a frame of its own"
LEDGER

echo "check-known-crashes.py:"

# 1-3. The three defects this repository actually owns, by their real (verdict, frame) pairs.
report "heap-use-after-free" "Distinct::Frame"
expect "a crash with its own frame resolves to its own issue" 0 "#3"

report "heap-use-after-free" "Shared::Frame"
expect "a shared frame under one verdict resolves to that verdict's issue" 0 "#1"

report "stack-overflow" "Shared::Frame"
expect "THE SAME FRAME under another verdict resolves elsewhere" 0 "#2"

# 4. THE LIVE LEDGER IS VALID, which is a different question from how matching behaves. This is
#    the only case touching the real file, and it asserts only that it loads and validates -- so
#    deleting #62's entries when #62 closes cannot make this suite red.
if python3 "$here/check-known-crashes.py" --check >/dev/null 2>&1; then
  ok "the committed ledger loads and every entry validates"
else
  bad "the committed ledger loads and every entry validates"
fi

# 5. A NEW DEFECT FAILS. The entire point: an unowned crash is what this nightly is for.
report "heap-buffer-overflow" "Nobody::FiledThis"
expect "an unowned crash is a new finding and fails" 1 "UNMATCHED"

# 6. A NEW DEFECT IN AN OWNED FUNCTION STILL FAILS, if the verdict differs. SEGV is uppercase,
#    which an earlier verdict pattern silently dropped -- and it is how a stack overflow surfaces
#    when ASan's own detector does not engage, i.e. #119's class.
report "SEGV" "Shared::Frame"
expect "a new verdict in an owned function still fails" 1 "UNMATCHED"

# 7. A LOG THAT IS NOT A CRASH REPORT is the caller's problem, and says so rather than guessing.
printf 'error[E0432]: unresolved import\nerror: could not compile\n' > "$work/report.log"
# EXIT 3, NOT 1: a log that is no crash report is the tool unable to classify, and until
# 2026-09-25 it shared exit 1 with a new finding, so the nightly could not tell them apart.
expect "a build failure is refused as not-a-crash-report, as a TOOLING exit" 3 "not a crash report"

# 8. AN UNRESOLVABLE REPORT IS NOT A FINDING. With no frames nothing can match, and calling that
#    a new defect announces a known one as new. Measured as a real defect in the first version.
printf '==1==ERROR: AddressSanitizer: heap-use-after-free on address 0x1\n    #0 0x55 (/nope/x+0x12)\n' \
  > "$work/report.log"
expect "a report whose frames cannot be resolved says so, as a TOOLING exit, not a finding" \
  3 "UNCLASSIFIED"

# 8b. THE TOOL ITSELF DYING IS NOT A FINDING. Python exits 1 on an uncaught exception, which was
#     the new-finding code, so a traceback announced a new defect. A log path that does not exist
#     raises inside the classifier; it must exit 3 and say the classifier raised.
set +e
out="$(python3 "$here/check-known-crashes.py" --ledger "$work/ledger.toml" \
  --log "$work/no-such-log.log" 2>&1)"
status=$?
set -e
if [ "$status" -eq 3 ] && grep -qF "the classifier itself raised" <<<"$out"; then
  ok "an uncaught exception in the classifier is a TOOLING exit (3), never a finding (1)"
else
  bad "an uncaught exception in the classifier is a TOOLING exit (3), never a finding (1) (exit $status)"
  sed 's/^/         /' <<<"$out" >&2
fi

# 8c-8e. THE FRAMES ARE THE REPORT'S, NOT THE FUZZER'S. libFuzzer prints `NEW_FUNC[..]: 0x… (…/t+0x…)`
#     every time it covers a new function; the classifier read offsets from the whole log, took the
#     first 40, and on 2026-09-20..25 symbolised six nights of coverage lines instead of #62's
#     stacks -- calling a known defect new. There are now TWO defences, each tested ALONE: every
#     case plants 250 decoys (more than MAX_FRAMES = 200, so the cap cannot rescue a broken
#     defence -- the first version of this case planted 45, and a review measured it passing with
#     both defences reverted) in a place only ONE defence excludes. A stub addr2line names each
#     offset, so no real binary is needed.
stub="$work/stubbin"
mkdir -p "$stub"
cat > "$stub/addr2line" <<'STUB'
#!/usr/bin/env bash
# `-f -C -e <binary> <addresses…>`: two lines per address, name then location.
shift 4
for address in "$@"; do
  if [ "$address" = "0xbeef" ]; then echo "Distinct::Frame"; else echo "Coverage::Noise$address"; fi
  echo "??:0"
done
STUB
chmod +x "$stub/addr2line"
: > "$work/fakebinary"

owned_frame_found() {
  local name="$1"
  set +e
  out="$(PATH="$stub:$PATH" python3 "$here/check-known-crashes.py" --ledger "$work/ledger.toml" \
    --log "$work/report.log" --binary "$work/fakebinary" --target t 2>&1)"
  status=$?
  set -e
  if [ "$status" -eq 0 ] && grep -qF "#3" <<<"$out"; then
    ok "$name"
  else
    bad "$name (exit $status)"
    sed 's/^/         /' <<<"$out" | head -4 >&2
  fi
}

# 8c. FRAME-SHAPED LINES BEFORE THE REPORT: only the slice (`report_of`) excludes them.
{
  for n in $(seq 1 250); do
    printf '    #%d 0x%x  (/w/fuzz/target/x/release/t+0x%x) (BuildId: ab)\n' "$n" "$n" "$((4096 + n))"
  done
  printf '==1==ERROR: AddressSanitizer: heap-use-after-free on address 0x1\n'
  printf '    #0 0x55  (/w/fuzz/target/x/release/t+0xbeef) (BuildId: ab)\n'
} > "$work/report.log"
owned_frame_found "frame-shaped lines before the report are not read: the slice starts at the banner"

# 8d. COVERAGE LINES INSIDE THE REPORT'S SLICE: only the frame-line anchor excludes them.
{
  printf '==1==ERROR: AddressSanitizer: heap-use-after-free on address 0x1\n'
  for n in $(seq 1 250); do
    printf '\tNEW_FUNC[1/1]: 0x%x  (/w/fuzz/target/x/release/t+0x%x) (BuildId: ab)\n' "$n" "$((4096 + n))"
  done
  printf '    #0 0x55  (/w/fuzz/target/x/release/t+0xbeef) (BuildId: ab)\n'
} > "$work/report.log"
owned_frame_found "coverage lines are never frames: offsets come from stack-frame lines only"

# 8e. THE VERDICT IS THE BANNER'S, NOT A PANIC MESSAGE'S. The CMap target's assertion formats its
#     whole input into the message printed BEFORE libFuzzer's banner; an input containing
#     `ERROR: libFuzzer: ` became the verdict and reached the public step summary (security
#     review, 2026-09-25). The message must neither set the verdict nor appear in the output.
{
  printf "thread '<unnamed>' (1) panicked at fuzz_targets/x.rs:1:2:\n"
  printf '"prefix ERROR: libFuzzer: SECRETINPUTBYTES more input"\n'
  printf '==42== ERROR: libFuzzer: deadly signal\n'
  printf '    #0 0x1 in Distinct::Frame(Thing)\n'
} > "$work/report.log"
set +e
out="$(python3 "$here/check-known-crashes.py" --ledger "$work/ledger.toml" --log "$work/report.log" 2>&1)"
status=$?
set -e
if grep -qF "verdict: deadly-signal" <<<"$out" && ! grep -qF "SECRETINPUTBYTES" <<<"$out"; then
  ok "a panic message carrying the marker neither sets the verdict nor reaches the output"
else
  bad "a panic message carrying the marker neither sets the verdict nor reaches the output (exit $status)"
  sed 's/^/         /' <<<"$out" | head -4 >&2
fi

# 9. A MULTI-WORD VERDICT. `deadly signal` was captured as `deadly`, so an entry spelling it
#    correctly could never have matched.
cat > "$work/phrase.toml" <<'LEDGER'
[[known]]
issue = 7
verdict = "deadly-signal"
frame = "Some::Frame"
note = "a libFuzzer verdict that is two words"
LEDGER
printf '==42== ERROR: libFuzzer: deadly signal\n    #0 0x1 in Some::Frame(Thing)\n' > "$work/report.log"
if out="$(python3 "$here/check-known-crashes.py" --ledger "$work/phrase.toml" --log "$work/report.log" 2>&1)" \
   && grep -qF "#7" <<<"$out"; then
  ok "a two-word verdict is normalised and matches its entry"
else
  bad "a two-word verdict is normalised and matches its entry"
  sed 's/^/         /' <<<"${out:-}" >&2
fi

# 10. AN ENTRY THE CLASSIFIER COULD NEVER RESOLVE IS REFUSED, not accepted in silence.
cat > "$work/bad.toml" <<'LEDGER'
[[known]]
issue = 7
verdict = "deadly signal"
frame = "Some::Frame"
note = "a space, so this can never match a normalised verdict"
LEDGER
if ! python3 "$here/check-known-crashes.py" --ledger "$work/bad.toml" --check >/dev/null 2>&1; then
  ok "a verdict that can never match is refused when the ledger is validated"
else
  bad "a verdict that can never match is refused when the ledger is validated"
fi

# 11. AN ENTRY WITH NO REASON IS REFUSED. An unexplained entry is a mute button.
cat > "$work/noreason.toml" <<'LEDGER'
[[known]]
issue = 7
verdict = "stack-overflow"
frame = "Some::Frame"
LEDGER
if ! python3 "$here/check-known-crashes.py" --ledger "$work/noreason.toml" --check >/dev/null 2>&1; then
  ok "an entry with no written reason is refused"
else
  bad "an entry with no written reason is refused"
fi

# 12. THE META-TEST. Break the pair in a copy beside the original -- match on frame alone -- and
#    require case 3 to resolve to the WRONG issue, which is what the pair prevents.
python3 - "$here/check-known-crashes.py" "$mutant" <<'PY'
import sys
from pathlib import Path
source = Path(sys.argv[1]).read_text(encoding="utf-8")
old = '        if e["verdict"] == verdict\n        and (\n'
assert old in source, "the matching rule moved; update this meta-test rather than deleting it"
mutated = source.replace(old, '        if (\n', 1)
assert mutated != source, "the mutation did not apply"
Path(sys.argv[2]).write_text(mutated, encoding="utf-8")
PY
report "stack-overflow" "QPDF::Doc::Pages::pushInheritedAttributesToPageInternal"
set +e
out="$(python3 "$mutant" --log "$work/report.log" 2>&1)"
status=$?
set -e
# The mutant never reaches classification: its PROBE GATE refuses first, naming the two fixtures
# that a frame-only match conflates. That is the gate doing its job, and it is the stronger
# result -- the rule is protected before any real log is looked at. The assertion is therefore
# "refuses, naming the conflation", not "mis-attributes".
if [ "$status" -ne 0 ] && grep -qF "DIFFERENT defect: matched" <<<"$out"; then
  ok "the probe gate refuses a frame-only match, naming the two defects it would conflate"
else
  bad "the probe gate refuses a frame-only match, naming the conflation (exit $status)"
  sed 's/^/         /' <<<"$out" >&2
fi

# 13-14. A PANIC KEY IS ONE OR THE OTHER, AND WHOLE (#285): an entry carrying both a frame and a
#    panic is ambiguous, and a panic with only its message would absorb every target's.
cat > "$work/both.toml" <<'LEDGER'
[[known]]
issue = 8
verdict = "deadly-signal"
frame = "Some::Frame"
panic_at = "fuzz_targets/compress.rs"
panic = "a message"
note = "both keys"
LEDGER
out="$(python3 "$here/check-known-crashes.py" --ledger "$work/both.toml" --check 2>&1 || true)"
if grep -qF "exactly one" <<<"$out"; then
  ok "an entry keyed on both a frame and a panic is refused"
else
  bad "an entry keyed on both a frame and a panic is refused"
  sed 's/^/         /' <<<"$out" >&2
fi
cat > "$work/half.toml" <<'LEDGER'
[[known]]
issue = 9
verdict = "deadly-signal"
panic = "a message"
note = "a message with no file"
LEDGER
out="$(python3 "$here/check-known-crashes.py" --ledger "$work/half.toml" --check 2>&1 || true)"
if grep -qF "either alone would absorb" <<<"$out"; then
  ok "a panic entry with no target file is refused"
else
  bad "a panic entry with no target file is refused"
  sed 's/^/         /' <<<"$out" >&2
fi

# 15-16. THE PANIC KEY'S OWN META-TESTS, each a copy beside the original whose probe gate must
#    refuse naming the probe it breaks: matching on the message alone (the file dropped), and a
#    pattern that does not allow the thread id current Rust prints -- the shape the first version
#    of this key was written in, which matched nothing in the real nightly log.
panic_mutant() {
  local name="$1" old="$2" new="$3" needle="$4"
  python3 - "$here/check-known-crashes.py" "$mutant" "$old" "$new" <<'PY'
import sys
from pathlib import Path
source = Path(sys.argv[1]).read_text(encoding="utf-8")
old, new = sys.argv[3], sys.argv[4]
assert source.count(old) == 1, f"the panic rule moved ({source.count(old)} matches); update this meta-test"
mutated = source.replace(old, new, 1)
assert mutated != source, "the mutation did not apply"
Path(sys.argv[2]).write_text(mutated, encoding="utf-8")
PY
  set +e
  out="$(python3 "$mutant" --check 2>&1)"
  status=$?
  set -e
  if [ "$status" -eq 3 ] && grep -qF "$needle" <<<"$out"; then
    ok "$name"
  else
    bad "$name (exit $status)"
    sed 's/^/         /' <<<"$out" >&2
  fi
}
panic_mutant "the probe gate refuses a panic key that ignores the target file" \
  'else (e["panic_at"], e["panic"]) in panics' \
  'else e["panic"] in {message for _, message in panics}' \
  "the same message from another target is a new finding"
panic_mutant "the probe gate refuses a panic pattern without the thread id" \
  "(?: \\(\\d+\\))?" \
  "" \
  "WITH A THREAD ID"

# 17. THE REAL LEDGER DOES NOT ABSORB BURROW'S OWN HANDLE MISUSE (#285). In a fuzz build, a
#    `qpdf_oh` qpdf does not hold panics with its own message; the #285 entries key on the
#    invariant failure's. A compress panic carrying the handle message must match nothing. Against
#    the real file on purpose: it is a claim about today's entries, and it stays true -- as an
#    unmatched crash -- when #285 closes and its entries go.
printf '%s\n' \
  "thread '<unnamed>' (1) panicked at fuzz_targets/compress.rs:179:13:" \
  "compress reported an internal error: qpdf was handed an object handle it does not hold (a burrow defect)" \
  "==1== ERROR: libFuzzer: deadly signal" \
  "    #0 0x1 in rust_panic" > "$work/handle.log"
set +e
out="$(python3 "$here/check-known-crashes.py" --log "$work/handle.log" 2>&1)"
status=$?
set -e
if [ "$status" -ne 0 ] && ! grep -qF "KNOWN" <<<"$out"; then
  ok "burrow's own handle misuse is not absorbed by the real #285 entries"
else
  bad "burrow's own handle misuse is not absorbed by the real #285 entries (exit $status)"
  sed 's/^/         /' <<<"$out" >&2
fi

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ] || exit 1
