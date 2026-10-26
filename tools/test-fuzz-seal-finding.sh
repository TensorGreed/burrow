#!/usr/bin/env bash
# Self-test for tools/fuzz-seal-finding.sh and tools/install-age.sh -- the encrypted-finding path of
# fuzz-nightly.yml (owner decision (62)(1)).
#
# Every case runs the REAL scripts over a synthetic finding with a THROWAWAY key generated here in a
# temp dir. No real key exists in or near this repository. The planted markers stand in for an
# input and an internal-error message; none is a real reproducer.
#
# Mutations are asserted to have applied before the suite that should catch them runs, and each
# mutant is a copy BESIDE the original (the script resolves install-age.sh from its own directory).
#
# Usage: tools/test-fuzz-seal-finding.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
seal="$here/fuzz-seal-finding.sh"
work="$(mktemp -d)"
mutant="$here/fuzz-seal-finding.MUTANT.sh"
trap 'rm -rf "$work" "$mutant"' EXIT

EXPECTED_CASES=19
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

# The pinned age, installed (checksum-verified) into a cache. This also exercises install-age.sh.
cache="${BURROW_AGE_CACHE:-$HOME/.cache/burrow-age}/$("$here/install-age.sh" --version)"
if [ ! -x "$cache/age" ] || [ ! -x "$cache/age-keygen" ]; then
  "$here/install-age.sh" "$cache" > /dev/null
fi
export AGE="$cache/age"
keygen="$cache/age-keygen"

# A throwaway keypair, and a second one that must NOT be able to decrypt.
"$keygen" -o "$work/key.txt" 2> /dev/null
"$keygen" -o "$work/other.txt" 2> /dev/null
pub="$("$keygen" -y "$work/key.txt")"
printf '# throwaway\n%s\n' "$pub" > "$work/recipient.pub"
printf '# placeholder copy\nPLACEHOLDER-NOT-A-KEY-REPLACE-WITH-age-keygen-PUBLIC-KEY\n' > "$work/placeholder.pub"

# A finding: an input carrying a marker, and an unredacted log carrying the marker line the fuzz
# helper writes (fuzz/src/lib.rs) plus a panic message and the location line.
INPUT_MARK="PLANTED-INPUT-MARKER-7f3a"
MSG_MARK="PLANTED-INTERNAL-MESSAGE-91bc"
mkdir -p "$work/artifacts/reorder"
printf '%%PDF-1.4 %s\n\x00\x01\x02' "$INPUT_MARK" > "$work/artifacts/reorder/crash-abc123"
cat > "$work/fuzz.log" <<LOG
INFO: Running with entropic power schedule (0xFF, 100).
#4362	NEW    cov: 4048 ft: 11219 corp: 365/1177Kb exec/s: 12 rss: 207Mb
BURROW-FUZZ-FINDING target=reorder internal-error: $MSG_MARK
thread '<unnamed>' (1) panicked at fuzz_targets/reorder.rs:287:13:
reorder reported an internal error: $MSG_MARK
==2850==ERROR: libFuzzer: deadly signal
SUMMARY: libFuzzer: deadly signal
LOG

run_seal() { # <script> <recipient> <out-dir> -> sets $rc, $note
  rc=0
  note="$(GITHUB_OUTPUT="$work/gh-output" "$1" reorder "$work/fuzz.log" "$2" "$3" "$work/artifacts/reorder" 2>&1)" || rc=$?
}

# 1-3. Seals, decrypts to the inputs and the log, and the upload directory holds only ciphertext.
: > "$work/gh-output"
run_seal "$seal" "$work/recipient.pub" "$work/up1"
if [ "$rc" -eq 0 ] && [ "$(find "$work/up1" -type f | wc -l)" -eq 1 ] && [ -f "$work/up1/reorder.age" ]; then
  ok "seals one file, <target>.age, and nothing else in the upload directory"
else bad "seal exit $rc; upload dir: $(find "$work/up1" 2>&1)"; fi

grep -q '^sealed=true$' "$work/gh-output" && ok "reports sealed=true" || bad "no sealed=true"

mkdir "$work/dec1"
"$AGE" -d -i "$work/key.txt" "$work/up1/reorder.age" | tar -x -C "$work/dec1"
if cmp -s "$work/dec1/bundle/fuzz.log" "$work/fuzz.log" \
  && cmp -s "$work/dec1/bundle/artifacts/crash-abc123" "$work/artifacts/reorder/crash-abc123"; then
  ok "decrypts with the throwaway key to the crash input and the unredacted log, byte for byte"
else bad "decrypted bundle differs from what was sealed"; fi

# 4. The ciphertext carries neither marker, and no plaintext file sits in the upload path.
leaked=0
for m in "$INPUT_MARK" "$MSG_MARK" "BURROW-FUZZ-FINDING" "%PDF-1.4"; do
  if grep -aqF "$m" "$work/up1/reorder.age"; then bad "ciphertext contains: $m"; leaked=1; fi
done
if [ "$leaked" -eq 0 ] && [ -z "$(find "$work/up1" -type f ! -name '*.age')" ]; then
  ok "no marker in the ciphertext; no non-.age file in the upload path"
fi

# 5. A different key cannot decrypt.
if "$AGE" -d -i "$work/other.txt" "$work/up1/reorder.age" > /dev/null 2>&1; then
  bad "a different private key decrypted the artifact"
else ok "a different private key cannot decrypt"; fi

# 6. THE PLACEHOLDER refuses: exit 4, nothing written.
: > "$work/gh-output"
run_seal "$seal" "$work/placeholder.pub" "$work/up2"
if [ "$rc" -eq 4 ] && [ ! -e "$work/up2" ] && grep -q '^sealed=false$' "$work/gh-output" \
  && grep -q "placeholder" <<< "$note"; then
  ok "the placeholder recipient exits 4, names the reason, and writes nothing"
else bad "placeholder: exit $rc, note: $note"; fi

# 7. The COMMITTED recipient file is the placeholder or a well-formed key, exactly one line.
committed="$repo/.github/fuzz-findings.recipient"
line="$(grep -vE '^[[:space:]]*(#|$)' "$committed" | tr -d '[:space:]')"
if [ "$(grep -vcE '^[[:space:]]*(#|$)' "$committed")" -eq 1 ] \
  && { [[ "$line" == PLACEHOLDER* ]] || grep -qE '^age1[a-z0-9]{58}$' <<< "$line"; }; then
  ok "the committed recipient file is the placeholder or one well-formed age key"
else bad "committed recipient file is neither"; fi

# 8. A malformed key is a failure (1), not a skip, and leaves nothing.
printf 'age1notakey\n' > "$work/bad.pub"
run_seal "$seal" "$work/bad.pub" "$work/up3"
if [ "$rc" -eq 1 ] && [ ! -e "$work/up3" ]; then ok "a malformed recipient fails closed (exit 1)"
else bad "malformed: exit $rc"; fi

# 9. Encryption failing leaves no file: a fake age that reports the pinned version, writes
#    PLAINTEXT to -o, then fails.
want="$("$here/install-age.sh" --version)"
cat > "$work/fake-age-fails" <<FAKE
#!/usr/bin/env bash
[ "\${1:-}" = "--version" ] && { echo "$want"; exit 0; }
while [ \$# -gt 0 ]; do [ "\$1" = "-o" ] && { cat "\${@: -1}" > "\$2"; }; shift; done
exit 1
FAKE
chmod +x "$work/fake-age-fails"
rc=0; note="$(AGE="$work/fake-age-fails" GITHUB_OUTPUT="$work/gh-output" "$seal" reorder "$work/fuzz.log" "$work/recipient.pub" "$work/up4" "$work/artifacts/reorder" 2>&1)" || rc=$?
if [ "$rc" -ne 0 ] && [ -z "$(find "$work/up4" -type f 2>/dev/null)" ]; then
  ok "an encryption failure fails the step and leaves nothing to upload"
else bad "failing age: exit $rc, files: $(find "$work/up4" -type f 2>&1)"; fi

# 10. An 'age' that SUCCEEDS but writes plaintext is refused by the header check.
cat > "$work/fake-age-plain" <<FAKE
#!/usr/bin/env bash
[ "\${1:-}" = "--version" ] && { echo "$want"; exit 0; }
while [ \$# -gt 0 ]; do [ "\$1" = "-o" ] && { cat "\${@: -1}" > "\$2"; }; shift; done
exit 0
FAKE
chmod +x "$work/fake-age-plain"
rc=0; note="$(AGE="$work/fake-age-plain" GITHUB_OUTPUT="$work/gh-output" "$seal" reorder "$work/fuzz.log" "$work/recipient.pub" "$work/up5" "$work/artifacts/reorder" 2>&1)" || rc=$?
if [ "$rc" -ne 0 ] && [ -z "$(find "$work/up5" -type f 2>/dev/null)" ] && grep -q "age header" <<< "$note"; then
  ok "an 'age' that writes plaintext is refused (header check) and leaves nothing"
else bad "plain-writing age: exit $rc, note: $note"; fi

# 11. A different age version is refused.
sed "s/$want/v0.0.0/" "$work/fake-age-plain" > "$work/fake-age-old"; chmod +x "$work/fake-age-old"
rc=0; note="$(AGE="$work/fake-age-old" GITHUB_OUTPUT="$work/gh-output" "$seal" reorder "$work/fuzz.log" "$work/recipient.pub" "$work/up6" "$work/artifacts/reorder" 2>&1)" || rc=$?
if [ "$rc" -eq 1 ] && grep -q "not the pinned" <<< "$note"; then ok "an unpinned age version is refused"
else bad "version: exit $rc, note: $note"; fi

# 12. A missing log, and a hostile target name.
rc=0; GITHUB_OUTPUT="$work/gh-output" "$seal" reorder "$work/nonexistent.log" "$work/recipient.pub" "$work/up7" "$work/artifacts/reorder" > /dev/null 2>&1 || rc=$?
r1=$rc; rc=0; GITHUB_OUTPUT="$work/gh-output" "$seal" '../x' "$work/fuzz.log" "$work/recipient.pub" "$work/up8" > /dev/null 2>&1 || rc=$?
if [ "$r1" -eq 1 ] && [ "$rc" -eq 1 ] && [ ! -e "$work/up7/reorder.age" ] && [ ! -e "$work/up8" ]; then
  ok "a missing log and a path-shaped target name both fail closed"
else bad "missing log exit $r1, hostile name exit $rc"; fi

# 13. A crash with NO input on disk (timeout/OOM) still seals the log.
rc=0; GITHUB_OUTPUT="$work/gh-output" "$seal" reorder "$work/fuzz.log" "$work/recipient.pub" "$work/up9" "$work/no-such-dir" > /dev/null 2>&1 || rc=$?
[ "$rc" -eq 0 ] && [ -f "$work/up9/reorder.age" ] && ok "a finding with no input file still seals the log" || bad "no-input seal: exit $rc"

# 14. MUTANT: the placeholder arm deleted. Applied, then the placeholder must stop exiting 4.
cp "$seal" "$mutant"
python3 - "$mutant" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
old = "  PLACEHOLDER*)"
assert old in s, "mutation did not apply"
open(p, "w").write(s.replace(old, "  NEVERMATCH*)", 1))
PY
cmp -s "$seal" "$mutant" && { bad "mutant 1 identical to the original"; }
run_seal "$mutant" "$work/placeholder.pub" "$work/up10"
[ "$rc" -ne 4 ] && ok "mutant without the placeholder arm no longer exits 4 (case 6 would catch it)" || bad "mutant still exits 4"

# 15. MUTANT: the header check deleted. The plaintext-writing age must then get through.
cp "$seal" "$mutant"
python3 - "$mutant" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
old = 'if [ "$header" != "age-encryption.org/v1" ]; then'
assert old in s, "mutation did not apply"
open(p, "w").write(s.replace(old, 'if false; then', 1))
PY
rc=0; AGE="$work/fake-age-plain" GITHUB_OUTPUT="$work/gh-output" "$mutant" reorder "$work/fuzz.log" "$work/recipient.pub" "$work/up11" "$work/artifacts/reorder" > /dev/null 2>&1 || rc=$?
[ "$rc" -eq 0 ] && grep -aqF "$MSG_MARK" "$work/up11/reorder.age" \
  && ok "mutant without the header check ships plaintext (case 10 would catch it)" || bad "header mutant not caught: exit $rc"
rm -f "$mutant"

# 16. install-age.sh refuses a tampered tarball and unpacks nothing.
case "$(uname -m)" in x86_64) arch=amd64 ;; *) arch=arm64 ;; esac
mkdir -p "$work/tamper/src/age"
printf '#!/bin/sh\necho %s\n' "$want" > "$work/tamper/src/age/age"
tar -czf "$work/tamper/age-$want-linux-$arch.tar.gz" -C "$work/tamper/src" age
rc=0; note="$(AGE_RELEASE_BASE="file://$work/tamper" "$here/install-age.sh" "$work/tamper/dest" 2>&1)" || rc=$?
if [ "$rc" -ne 0 ] && grep -q "refusing to unpack" <<< "$note" && [ ! -e "$work/tamper/dest" ]; then
  ok "install-age refuses a tarball whose sha256 is not the pin, and unpacks nothing"
else bad "tampered tarball: exit $rc, note: $note"; fi

# 17. The redacted public log carries the diagnosis but NOT the internal-error message.
red="$(python3 "$repo/tools/redact-fuzz-log.py" < "$work/fuzz.log" 2> /dev/null)"
if ! grep -qF "$MSG_MARK" <<< "$red" && ! grep -qF "BURROW-FUZZ-FINDING" <<< "$red" \
  && grep -qF "deadly signal" <<< "$red" && grep -qF "panicked at fuzz_targets/reorder.rs:287:13:" <<< "$red"; then
  ok "redacted log: message and marker line withheld, verdict and panic location kept"
else bad "redacted log: $red"; fi

# 18. The marker format in the test is the one the helper writes.
grep -qF 'BURROW-FUZZ-FINDING target={target} internal-error: {message}' "$repo/fuzz/src/lib.rs" \
  && ok "the marker line planted here is the format fuzz/src/lib.rs writes" || bad "helper format drifted from the planted marker"

# 19. WORKFLOW WIRING: the nightly seals with this script, uploads only a .age path with a short
#     retention, and the published-reproducer gate still accepts it.
python3 - "$repo/.github/workflows/fuzz-nightly.yml" <<'PY' && wired=1 || wired=0
import sys, yaml
wf = yaml.safe_load(open(sys.argv[1]))
steps = wf["jobs"]["fuzz"]["steps"]
uploads = [s for s in steps if str(s.get("uses", "")).lower().startswith("actions/upload-artifact")]
assert len(uploads) == 1, f"expected exactly one upload step, found {len(uploads)}"
up = uploads[0]
path = str(up["with"]["path"]).strip()
assert path.endswith(".age"), f"upload path does not end in .age: {path}"
assert "fuzz/artifacts" not in path and "fuzz.log" not in path
assert 1 <= int(up["with"]["retention-days"]) <= 14, "retention must be short"
assert up["with"].get("if-no-files-found") == "error"
seal = [s for s in steps if "tools/fuzz-seal-finding.sh" in str(s.get("run", ""))]
assert len(seal) == 1 and ".github/fuzz-findings.recipient" in seal[0]["run"]
install = [s for s in steps if "tools/install-age.sh" in str(s.get("run", ""))]
assert len(install) == 1
order = [steps.index(install[0]), steps.index(seal[0]), steps.index(up)]
assert order == sorted(order), "install, seal, upload must run in that order"
assert "steps.seal.outputs.sealed == 'true'" in str(up["if"])
PY
if [ "$wired" -eq 1 ] && python3 "$repo/tools/check-no-published-reproducers.py" > /dev/null 2>&1; then
  ok "fuzz-nightly.yml: install -> seal -> upload of one .age path, short retention; reproducer gate accepts it"
else bad "workflow wiring (wired=$wired) or the reproducer gate"; fi

echo "fuzz-seal-finding: $pass passed, $fail failed (expected $EXPECTED_CASES cases)"
[ "$fail" -eq 0 ] && [ "$pass" -eq "$EXPECTED_CASES" ]
