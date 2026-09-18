#!/usr/bin/env bash
# Test that redact-fuzz-log.py withholds a crashing input and keeps the diagnosis.
#
# The fixture below is the SHAPE of a real leaking log: cargo-fuzz's `std::fmt::Debug` dump (the
# whole input as decimal bytes, no size limit), libFuzzer's hex/ASCII/Base64 of a small unit, and
# a progress line whose `DE:` dictionary entry is a literal fragment of the input. All three were
# measured in the 2026-09-18 nightly log, from which all 16,208 bytes of #119's reproducer were
# recoverable. The bytes here are a harmless stand-in: no real reproducer belongs in this tree.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
mutant="$here/redact-fuzz-log.MUTANT.py"
trap 'rm -rf "$work" "$mutant"' EXIT

EXPECTED_CASES=5
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

cat > "$work/leaky.log" <<'LOG'
INFO: Running with entropic power schedule
#4362	NEW    cov: 4048 ft: 11219 corp: 365/1177Kb exec/s: 12 rss: 207Mb MS: 3 CrossOver- DE: "SECRETMARKER"
==2850==ERROR: AddressSanitizer: stack-overflow on address 0x7ffc70848bd8
    #7 0x55f99897b9cc  (/home/runner/release/rotate+0x5589cc)
SUMMARY: AddressSanitizer: stack-overflow (/home/runner/release/rotate+0x28f043)
MS: 1 CrossOver-; base unit: e9b8e4b5376dad59bc2beb453e3c581a0c670d8a
0x53,0x45,0x43,0x52,0x45,0x54,
SECRETMARKER 1 0 obj << /Type /Catalog
artifact_prefix='/home/runner/fuzz/artifacts/rotate/'; Test unit written to /home/runner/fuzz/artifacts/rotate/crash-46610c95
Base64: U0VDUkVUTUFSS0VS
Output of `std::fmt::Debug`:

	[83, 69, 67, 82, 69, 84, 77, 65, 82, 75, 69, 82]

Reproduce with:
LOG

out="$(python3 "$here/redact-fuzz-log.py" < "$work/leaky.log" 2>/dev/null)"

# 1. NOTHING THAT CARRIES THE INPUT SURVIVES. One assertion per leak channel, each named, so a
#    failure says which channel reopened rather than only that something did.
leaked=0
for channel in "SECRETMARKER" "Base64:" "std::fmt::Debug" "83, 69, 67" "0x53,0x45" "DE:"; do
  if grep -qF "$channel" <<<"$out"; then
    bad "the redacted log still carries: $channel"
    leaked=1
  fi
done
[ "$leaked" -eq 0 ] && ok "no channel carrying the input survives redaction"

# 2. ...AND THE DIAGNOSIS DOES. A redactor that drops everything passes case 1 and is useless.
kept=1
for signal in "AddressSanitizer: stack-overflow" "SUMMARY:" "rotate+0x5589cc" "cov: 4048"; do
  if ! grep -qF "$signal" <<<"$out"; then
    bad "redaction removed the diagnosis too: $signal"
    kept=0
  fi
done
[ "$kept" -eq 1 ] && ok "the verdict, the frame and the counters survive"

# 3. THE PROGRESS LINE IS TRIMMED, NOT DROPPED -- it carries counters and then a dictionary entry.
if grep -q "cov: 4048" <<<"$out" && ! grep -q "SECRETMARKER" <<<"$out"; then
  ok "a partly-safe line is trimmed at the input-derived tail"
else
  bad "a partly-safe line is trimmed at the input-derived tail"
fi

# 4. THE UNREDACTED LOG IS STILL AVAILABLE ON THE RUNNER, or the next step cannot diagnose.
python3 "$here/redact-fuzz-log.py" --tee "$work/full.log" < "$work/leaky.log" >/dev/null 2>&1
if grep -qF "SECRETMARKER" "$work/full.log" && [ "$(wc -l < "$work/full.log")" -eq "$(wc -l < "$work/leaky.log")" ]; then
  ok "--tee keeps the complete log for local use"
else
  bad "--tee keeps the complete log for local use"
fi

# 5. THE META-TEST. Break the allowlist in a copy beside the original; the probe gate must refuse
#    and name the channel that would leak.
python3 - "$here/redact-fuzz-log.py" "$mutant" <<'PY'
import sys
from pathlib import Path
source = Path(sys.argv[1]).read_text(encoding="utf-8")
old = 'TRUNCATE_AT = (" MS: ", " DE: ")'
assert old in source, "the truncation rule moved; update this meta-test rather than deleting it"
mutated = source.replace(old, 'TRUNCATE_AT = ()', 1)
assert mutated != source, "the mutation did not apply"
Path(sys.argv[2]).write_text(mutated, encoding="utf-8")
PY
set +e
probe_out="$(python3 "$mutant" --probe 2>&1)"
probe_status=$?
set -e
if [ "$probe_status" -ne 0 ] && grep -qF "survived trimming" <<<"$probe_out"; then
  ok "the probe gate refuses a broken truncation rule, naming what would leak"
else
  bad "the probe gate refuses a broken truncation rule, naming what would leak (exit $probe_status)"
  sed 's/^/         /' <<<"$probe_out" >&2
fi

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ] || exit 1
