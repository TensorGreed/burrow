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
# ONE rustc for every probe build. The probes compile an rlib from the repo and then a bin in a temp
# directory; bare `rustc` resolves per working directory, so the repo's rust-toolchain.toml pinned
# the rlib while the temp dir got the runner's default, and rustc refused the mismatch (E0514) in CI
# only. Resolve it once, from the repo, and use that path for both.
rustc_bin="$(cd "$repo" && rustup which rustc 2> /dev/null || command -v rustc)"
# Every probe build goes through probe_rustc. On failure it prints rustc's first lines (indented, to stderr) so a CI
# failure names its cause. Safe to print: the sources are the repo's own fuzz/src/lib.rs, a mutant of it, and fixed
# probe strings written by this script, never fuzz input.
probe_rustc() {
  "$rustc_bin" "$@" > /dev/null 2> "$work/rustc.err" && return 0
  { echo "  probe build failed (rustc $rustc_bin):"; head -n 4 "$work/rustc.err" | sed 's/^/    /'; } >&2
  return 1
}
mutant="$here/fuzz-seal-finding.MUTANT.sh"
trap 'rm -rf "$work" "$mutant"' EXIT

EXPECTED_CASES=29
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

# 6. A PLACEHOLDER, MISSING or EMPTY recipient FAILS (owner decision (66)(4)): exit 1, the
#    reason named, nothing written to the upload path, never a pass.
: > "$work/empty.pub"
printf '# only a comment\n' > "$work/comment-only.pub"
bad6=""
i=0
for spec in "placeholder:$work/placeholder.pub:still the placeholder" \
  "missing:$work/no-such.pub:no recipient file" \
  "empty:$work/empty.pub:exactly one recipient" \
  "comment-only:$work/comment-only.pub:exactly one recipient"; do
  name="${spec%%:*}"; rest="${spec#*:}"; file="${rest%%:*}"; reason="${rest#*:}"
  i=$((i + 1)); : > "$work/gh-output"
  run_seal "$seal" "$file" "$work/up2-$i"
  if [ "$rc" -ne 1 ] || [ -e "$work/up2-$i" ] || grep -q '^sealed=true$' "$work/gh-output" \
    || ! grep -q "$reason" <<< "$note"; then
    bad6="$bad6 [$name: exit $rc, note: $note]"
  fi
done
if [ -z "$bad6" ]; then
  ok "placeholder, missing, empty and comment-only recipients each exit 1, name the reason, write nothing"
else bad "recipient refusals:$bad6"; fi

# 7. The COMMITTED recipient file is one well-formed key, and NOT the placeholder.
committed="$repo/.github/fuzz-findings.recipient"
line="$(grep -vE '^[[:space:]]*(#|$)' "$committed" | tr -d '[:space:]')"
if [ "$(grep -vcE '^[[:space:]]*(#|$)' "$committed")" -eq 1 ] && grep -qE '^age1[a-z0-9]{58}$' <<< "$line"; then
  ok "the committed recipient file is one well-formed age key, not the placeholder"
else bad "committed recipient file is not one well-formed age key"; fi

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

# 14. MUTANT: the placeholder arm deleted. Applied; the placeholder then falls to the malformed-key
#     check, so the reason named is no longer "placeholder" (case 6 asserts the reason).
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
if ! grep -q "still the placeholder" <<< "$note"; then
  ok "mutant without the placeholder arm no longer names the placeholder (case 6 would catch it)"
else bad "mutant still names the placeholder"; fi

# 14b. MUTANT: a missing recipient passes (exit 0). Applied; case 6 requires exit 1 for it.
cp "$seal" "$mutant"
python3 - "$mutant" <<'PY'
import sys
p = sys.argv[1]
s = open(p).read()
old = 'status_note "no recipient file at $recipient_file"; exit 1;'
assert old in s, "mutation did not apply"
open(p, "w").write(s.replace(old, 'status_note "no recipient file at $recipient_file"; exit 0;', 1))
PY
cmp -s "$seal" "$mutant" && { bad "mutant 14b identical to the original"; }
run_seal "$mutant" "$work/no-such.pub" "$work/up10b"
if [ "$rc" -eq 0 ]; then
  ok "mutant that passes on a missing recipient exits 0 (case 6 requires exit 1, so it would catch it)"
else bad "missing-recipient mutant: exit $rc"; fi

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
grep -qF 'BURROW-FUZZ-FINDING target={target} internal-error: {escaped}"' "$repo/fuzz/src/lib.rs" \
  && grep -qF 'let escaped = message.escape_debug();' "$repo/fuzz/src/lib.rs" \
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
# (66)(4): a seal failure is a red step -- no warn-and-pass, no exit-code-4 branch, no continue-on-error.
sr = str(seal[0]["run"])
assert "exit 0" not in sr and "-eq 4" not in sr and "::warning" not in sr, "seal step must not swallow a failure"
assert not seal[0].get("continue-on-error"), "seal step must not continue-on-error"
# (66)(4): every crash is sealed. Both steps run under always() on any failed fuzz step or non-zero
# status, and neither is gated on the classifier's verdict (ledger-matched or not).
for s in (install[0], seal[0]):
    cond = str(s["if"])
    assert cond.lstrip().startswith("always()"), f"{s['name']}: not always()-gated"
    assert not any(w in cond.lower() for w in ("classif", "matched", "ledger", "unmatched")), \
        f"{s['name']}: filters on the classifier verdict"
assert "steps.fuzz.outputs.status != '0'" in str(install[0]["if"]) and "steps.fuzz.outcome == 'failure'" in str(install[0]["if"])
PY
if [ "$wired" -eq 1 ] && python3 "$repo/tools/check-no-published-reproducers.py" > /dev/null 2>&1; then
  ok "fuzz-nightly.yml: install -> seal -> upload of one .age path, short retention; reproducer gate accepts it"
else bad "workflow wiring (wired=$wired) or the reproducer gate"; fi

# 20. #[track_caller] on the helper: a target that panics through burrow_fuzz::internal_error gets
#     a panic line naming ITS file, not fuzz/src/lib.rs -- tools/check-known-crashes.py keys
#     panic_at on that file. The REAL lib.rs is compiled into a probe "target"; the mutant is a
#     copy of it with the attribute removed (asserted applied) and must name lib.rs instead.
probe_dir="$work/track"; mkdir -p "$probe_dir/fuzz_targets"
cat > "$probe_dir/fuzz_targets/probe.rs" <<'RS'
fn main() { burrow_fuzz::internal_error("probe", "PLANTED-MSG"); }
RS
panic_line() { # <lib.rs> -> prints the panic location and the line after it
  # Run from probe_dir with a relative source path, as cargo does, so the location is the bare
  # `fuzz_targets/probe.rs` the classifier sees in a nightly log.
  (cd "$probe_dir" && probe_rustc --edition 2021 --crate-type bin --crate-name probe --extern "burrow_fuzz=$2" \
    -o probe fuzz_targets/probe.rs || exit 9
  ./probe 2>&1 | grep -A1 'panicked at' | tr '\n' '|'; true)
}
probe_rustc --edition 2021 --crate-type rlib --crate-name burrow_fuzz -o "$probe_dir/libreal.rlib" "$repo/fuzz/src/lib.rs" \
  && real="$(panic_line x "$probe_dir/libreal.rlib")" || real="BUILD-FAILED"
sed '/^#\[track_caller\]$/d' "$repo/fuzz/src/lib.rs" > "$probe_dir/lib_mutant.rs"
cmp -s "$repo/fuzz/src/lib.rs" "$probe_dir/lib_mutant.rs" && bad "track_caller mutation did not apply"
probe_rustc --edition 2021 --crate-type rlib --crate-name burrow_fuzz -o "$probe_dir/libmutant.rlib" "$probe_dir/lib_mutant.rs" \
  && mut="$(panic_line x "$probe_dir/libmutant.rlib")" || mut="BUILD-FAILED"
if grep -q 'panicked at fuzz_targets/probe.rs:[0-9]*:[0-9]*:|probe reported an internal error: PLANTED-MSG|' <<< "$real" \
  && grep -q 'panicked at .*lib_mutant.rs' <<< "$mut" && ! grep -q 'fuzz_targets/probe.rs' <<< "$mut"; then
  ok "internal_error's panic line names the calling target file, directly above the message; without #[track_caller] it names the helper's file"
else bad "track_caller: real=[$real] mutant=[$mut]"; fi

# 21. THE MARKER AND THE PANIC LINE ARE EACH ONE LINE (owner (72)). A message with a newline must not
#     start a second log line: the redactor allowlists by line prefix, so "SUMMARY: leaked" at the
#     start of a line would be published. The REAL lib.rs is compiled into a probe; there are two
#     mutants, each dropping ONE of the two escapes (asserted applied).
#     WHAT THIS PROVES: with the real helper the whole stderr has exactly one marker line and one
#     panic line, no line begins "SUMMARY: leaked", and the full stderr piped through the redactor
#     publishes nothing containing "leaked"; the marker-only mutant and the panic-only mutant each
#     leave a line-start "SUMMARY: leaked" and the redactor publishes it.
#     WHAT IT DOES NOT PROVE: a target that panics with input in a message by a route other than
#     internal_error (documented in tools/check-known-crashes.py).
esc_dir="$work/esc"; mkdir -p "$esc_dir/fuzz_targets"
cat > "$esc_dir/fuzz_targets/probe.rs" <<'RS'
fn main() { burrow_fuzz::internal_error("probe", "PRE\nSUMMARY: leaked\rTAIL"); }
RS
esc_run() { # <rlib> -> stderr of the probe built against it, in $esc_dir/stderr.txt
  (cd "$esc_dir" && probe_rustc --edition 2021 --crate-type bin --crate-name probe --extern "burrow_fuzz=$1" \
    -o probe fuzz_targets/probe.rs && { ./probe > stderr.txt 2>&1 || true; }) || echo BUILD-FAILED > "$esc_dir/stderr.txt"
}
esc_check() { # <stderr file> -> echoes marker lines|panic lines|line-start SUMMARY lines|published "leaked" lines
  local f="$1" n_marker n_panic n_sum n_pub
  n_marker="$(grep -c '^BURROW-FUZZ-FINDING' "$f" || true)"
  n_panic="$(grep -c 'probe reported an internal error: ' "$f" || true)"
  n_sum="$(grep -c '^SUMMARY: leaked' "$f" || true)"
  n_pub="$(python3 "$repo/tools/redact-fuzz-log.py" < "$f" 2> /dev/null | grep -c 'leaked' || true)"
  echo "$n_marker|$n_panic|$n_sum|$n_pub"
}
esc_build() { # <lib.rs> <name> -> echoes esc_check of a probe built against it
  probe_rustc --edition 2021 --crate-type rlib --crate-name burrow_fuzz -o "$esc_dir/lib$2.rlib" "$1" \
    && esc_run "$esc_dir/lib$2.rlib" || echo BUILD-FAILED > "$esc_dir/stderr.txt"
  esc_check "$esc_dir/stderr.txt"
}
real_chk="$(esc_build "$repo/fuzz/src/lib.rs" real)"
panic_ln="$(grep 'probe reported an internal error: ' "$esc_dir/stderr.txt" | head -1 || true)"
# Mutant A drops the MARKER's escape; mutant B drops the PANIC line's.
sed 's/internal-error: {escaped}"/internal-error: {message}"/' "$repo/fuzz/src/lib.rs" > "$esc_dir/lib_mut_marker.rs"
cmp -s "$repo/fuzz/src/lib.rs" "$esc_dir/lib_mut_marker.rs" && bad "marker escape mutation did not apply"
grep -qF 'internal-error: {message}"' "$esc_dir/lib_mut_marker.rs" || bad "marker mutation did not produce the expected text"
sed 's/internal error: {escaped}")/internal error: {message}")/' "$repo/fuzz/src/lib.rs" > "$esc_dir/lib_mut_panic.rs"
cmp -s "$repo/fuzz/src/lib.rs" "$esc_dir/lib_mut_panic.rs" && bad "panic-line escape mutation did not apply"
grep -qF 'internal error: {message}")' "$esc_dir/lib_mut_panic.rs" || bad "panic mutation did not produce the expected text"
mut_marker="$(esc_build "$esc_dir/lib_mut_marker.rs" mutmarker)"
mut_panic="$(esc_build "$esc_dir/lib_mut_panic.rs" mutpanic)"
if [ "$real_chk" = "1|1|0|0" ] \
  && [ "$panic_ln" = 'probe reported an internal error: PRE\nSUMMARY: leaked\rTAIL' ] \
  && [ "$(cut -d'|' -f3-4 <<< "$mut_marker")" = "1|1" ] \
  && [ "$(cut -d'|' -f3-4 <<< "$mut_panic")" = "1|1" ]; then
  ok "internal_error: marker and panic line are each one escaped line, nothing published from either; dropping either escape leaves a published line"
else bad "escape: real=[$real_chk] panic=[$panic_ln] mut_marker=[$mut_marker] mut_panic=[$mut_panic]"; fi

# 22-23. THE SAME CAPTURE IN fuzz-reproduce.yml AND ci.yml's `fuzz smoke` (owner (96), nit 3 of (72)).
#     wiring.py checks one workflow job: exactly one upload, its path a single `.age` under the sealed
#     directory (never fuzz/artifacts or a log), short retention, if-no-files-found error; install ->
#     seal -> upload in that order; the seal step calls the script with the committed recipient, is not
#     continue-on-error and swallows nothing ((66)(4)); install and seal are always()-gated and not
#     filtered on any classifier verdict. It prints "examined <n> steps" so the count is a
#     measurement. Each workflow then gets four mutants, each asserted applied, each of which must be
#     refused with its reason named.
cat > "$work/wiring.py" <<'PY'
import sys, yaml
path, job, kind = sys.argv[1:4]
wf = yaml.safe_load(open(path))
steps = wf["jobs"][job]["steps"]
def run_of(s): return str(s.get("run", ""))
uploads = [s for s in steps if str(s.get("uses", "")).lower().startswith("actions/upload-artifact")]
assert len(uploads) == 1, f"expected exactly one upload step, found {len(uploads)}"
up = uploads[0]
upath = str(up["with"]["path"]).strip()
assert upath.endswith(".age"), f"upload path does not end in .age: {upath}"
assert "fuzz/artifacts" not in upath and ".log" not in upath, f"upload path names inputs or a log: {upath}"
assert 1 <= int(up["with"]["retention-days"]) <= 14, "retention must be short"
assert up["with"].get("if-no-files-found") == "error", "if-no-files-found must be error"
seal = [s for s in steps if "tools/fuzz-seal-finding.sh" in run_of(s)]
assert len(seal) == 1 and ".github/fuzz-findings.recipient" in run_of(seal[0]), "seal step missing or without the committed recipient"
install = [s for s in steps if "tools/install-age.sh" in run_of(s)]
assert len(install) == 1, "install step missing"
order = [steps.index(install[0]), steps.index(seal[0]), steps.index(up)]
assert order == sorted(order), "install, seal, upload must run in that order"
assert "steps.seal.outputs.sealed == 'true'" in str(up["if"]), "upload not gated on the seal"
assert seal[0].get("id") == "seal" and "steps.age.outcome == 'success'" in str(seal[0]["if"]), "seal not gated on the install"
sr = run_of(seal[0])
assert "exit 0" not in sr and "-eq 4" not in sr and "::warning" not in sr, "seal step swallows a failure"
assert not seal[0].get("continue-on-error"), "seal step continues on error"
for st in (install[0], seal[0]):
    cond = str(st["if"])
    assert cond.lstrip().startswith("always()"), f"{st['name']}: not always()-gated"
    assert not any(w in cond.lower() for w in ("classif", "matched", "ledger", "unmatched")), f"{st['name']}: filters on a verdict"
if kind == "smoke":
    name = [s for s in steps if s.get("id") == "finding"]
    assert len(name) == 1 and "fuzz-failed-target" in run_of(name[0]), "the step naming the failing target is missing"
    assert "steps.finding.outputs.target != ''" in str(install[0]["if"]), "install not gated on a named target"
    writers = [s for s in steps if "fuzz-failed-target" in run_of(s) and 'exit "$status"' in run_of(s)]
    assert len(writers) == 2, f"both fuzz loops must name the failing target, found {len(writers)}"
else:
    assert "steps.fuzz.outputs.target != ''" in str(install[0]["if"]), "install not gated on a validated target"
    assert "steps.fuzz.outputs.status != '0'" in str(install[0]["if"]) and "steps.fuzz.outcome == 'failure'" in str(install[0]["if"]), "install not gated on a crash"
print(f"examined {len(steps)} steps of {job}; 1 upload, 1 seal, 1 install")
PY
wire() { # <workflow-file> <job> <kind> -> rc, note
  rc=0; note="$(python3 "$work/wiring.py" "$1" "$2" "$3" 2>&1)" || rc=$?
}
mutate_wf() { # <src> <dst> <old> <new>: a copy with one replacement, asserted applied
  python3 - "$@" <<'PY'
import sys
src, dst, old, new = sys.argv[1:5]
s = open(src).read()
assert old in s, "mutation did not apply"
open(dst, "w").write(s.replace(old, new, 1))
PY
}
wfdir="$repo/.github/workflows"
m_label=(upload-artifacts-dir seal-continues seal-not-always long-retention)
m_old=("path: \${{ runner.temp }}/sealed/" "id: seal" "if: always() && steps.age.outcome == 'success'" $'.age\n          retention-days: 7')
m_new=("path: fuzz/artifacts/" $'id: seal\n        continue-on-error: true' "if: steps.age.outcome == 'success'" $'.age\n          retention-days: 90')
m_why=("names inputs or a log" "seal step continues on error" "not always()-gated" "retention must be short")
for spec in "fuzz-reproduce.yml reproduce repro" "ci.yml fuzz smoke"; do
  read -r wfile wjob wkind <<< "$spec"
  wire "$wfdir/$wfile" "$wjob" "$wkind"
  real_note="$note"; real_rc=$rc
  mut_ok=1; mut_names=""; mut_n=0
  for i in 0 1 2 3; do
    if ! mutate_wf "$wfdir/$wfile" "$work/mut.yml" "${m_old[$i]}" "${m_new[$i]}" 2> /dev/null; then
      mut_ok=0; mut_names="$mut_names [${m_label[$i]}: did not apply]"; continue
    fi
    cmp -s "$wfdir/$wfile" "$work/mut.yml" && { mut_ok=0; mut_names="$mut_names [${m_label[$i]}: identical]"; continue; }
    wire "$work/mut.yml" "$wjob" "$wkind"
    if [ "$rc" -eq 0 ] || ! grep -qF "${m_why[$i]}" <<< "$note"; then
      mut_ok=0; mut_names="$mut_names [${m_label[$i]}: rc=$rc note=$note]"
    else mut_n=$((mut_n + 1)); fi
  done
  if [ "$real_rc" -eq 0 ] && grep -q '^examined [0-9]* steps' <<< "$real_note" && [ "$mut_ok" -eq 1 ] && [ "$mut_n" -eq 4 ]; then
    ok "$wfile ($wjob): $real_note; $mut_n of 4 mutants (artifacts dir, continue-on-error, no always(), long retention) refused by name"
  else bad "$wfile wiring: rc=$real_rc note=[$real_note] mutants caught $mut_n of 4:$mut_names"; fi
done
rm -f "$work/mut.yml"

# 24-25. THE SMOKE JOB'S NAMING STEP, run for real: the step's own script from ci.yml, over a temp
#     RUNNER_TEMP. No failed-target file (no crash) -> no `target=` output, so install/seal/upload
#     skip and no artifact exists; a file naming a target -> that target; a file naming something
#     that is not a plain name -> the step fails and writes no output. Then a mutant without the
#     file guard (asserted applied) must misbehave on a clean run.
python3 - "$wfdir/ci.yml" > "$work/finding.sh" <<'PY'
import sys, yaml
steps = yaml.safe_load(open(sys.argv[1]))["jobs"]["fuzz"]["steps"]
print([s for s in steps if s.get("id") == "finding"][0]["run"])
PY
run_finding() { # <script> <content|NONE> -> rc, outs
  rm -rf "$work/rt" "$work/ghout"; mkdir -p "$work/rt"; : > "$work/ghout"
  [ "$2" = NONE ] || printf '%s\n' "$2" > "$work/rt/fuzz-failed-target"
  rc=0; RUNNER_TEMP="$work/rt" GITHUB_OUTPUT="$work/ghout" bash "$1" > /dev/null 2>&1 || rc=$?
  outs="$(cat "$work/ghout")"
}
run_finding "$work/finding.sh" NONE; a_rc=$rc; a_out="$outs"
run_finding "$work/finding.sh" reorder; b_rc=$rc; b_out="$outs"
run_finding "$work/finding.sh" 'x; echo PLANTED-MSG'; c_rc=$rc; c_out="$outs"
if [ "$a_rc" -eq 0 ] && [ -z "$a_out" ] && [ "$b_rc" -eq 0 ] && [ "$b_out" = "target=reorder" ] \
  && [ "$c_rc" -ne 0 ] && [ -z "$c_out" ]; then
  ok "smoke naming step: no crash -> no output (nothing sealed or uploaded); a crash -> its target; a non-plain name -> red, no output"
else bad "naming step: none=[$a_rc/$a_out] reorder=[$b_rc/$b_out] bad=[$c_rc/$c_out]"; fi

mutate_wf "$work/finding.sh" "$work/finding-mut.sh" 'if [ -f "$failed" ]; then' 'if true; then'
cmp -s "$work/finding.sh" "$work/finding-mut.sh" && bad "naming-step mutation identical"
run_finding "$work/finding-mut.sh" NONE
if [ "$rc" -ne 0 ] || [ -n "$outs" ]; then
  ok "mutant naming step without the file guard misbehaves on a clean run (the no-crash case above would catch it)"
else bad "naming-step mutant behaved on a clean run"; fi

# 26. A real crash through the real seal script, called the way the smoke job calls it (per-target log
#     fuzz-<target>.log, the target's own artifacts dir). One .age, no plaintext in it, and neither
#     marker in the script's own output (which is what the job log would show).
mkdir -p "$work/smoke/fuzz/artifacts/reorder" "$work/smoke/rt"
cp "$work/artifacts/reorder/"* "$work/smoke/fuzz/artifacts/reorder/"
cp "$work/fuzz.log" "$work/smoke/rt/fuzz-reorder.log"
rc=0; smoke_out="$(cd "$work/smoke" && GITHUB_OUTPUT="$work/smoke/gh" "$seal" reorder rt/fuzz-reorder.log "$work/recipient.pub" rt/sealed fuzz/artifacts/reorder 2>&1)" || rc=$?
if [ "$rc" -eq 0 ] && [ "$(find "$work/smoke/rt/sealed" -type f | wc -l)" -eq 1 ] \
  && ! grep -aqF "$INPUT_MARK" "$work/smoke/rt/sealed/reorder.age" && ! grep -aqF "$MSG_MARK" "$work/smoke/rt/sealed/reorder.age" \
  && ! grep -qF "$INPUT_MARK" <<< "$smoke_out" && ! grep -qF "$MSG_MARK" <<< "$smoke_out"; then
  ok "smoke-shaped call: one .age, no plaintext in it, neither marker in the script's output"
else bad "smoke-shaped call: exit $rc"; fi

# 27. The published-reproducer gate still passes and now counts fuzz-reproduce.yml's upload.
gate_rc=0; gate_out="$(python3 "$repo/tools/check-no-published-reproducers.py" 2>&1)" || gate_rc=$?
if [ "$gate_rc" -eq 0 ] && grep -qE 'fuzz-reproduce.yml: [0-9]+ step\(s\), 1 upload' <<< "$gate_out"; then
  ok "check-no-published-reproducers accepts the new uploads and counts fuzz-reproduce.yml's"
else bad "reproducer gate (exit $gate_rc): $gate_out"; fi

# 28. Privacy of the five new sealing steps themselves (3 in smoke, 2 in reproduce): none prints a log or traces.
python3 - "$wfdir/ci.yml" "$wfdir/fuzz-reproduce.yml" <<'PY' && priv=1 || priv=0
import sys, yaml, re
n = 0
for path, job in ((sys.argv[1], "fuzz"), (sys.argv[2], "reproduce")):
    for st in yaml.safe_load(open(path))["jobs"][job]["steps"]:
        if st.get("name") in ("Name the failing target", "Install the pinned age", "Encrypt the finding to the owner's key"):
            r = str(st["run"]); n += 1
            assert not re.search(r"\b(cat|tail|head|echo|printf)\b[^\n]*fuzz[-.\w]*\.log", r), f"{st['name']} prints a log"
            assert "set -x" not in r and "::debug" not in r
assert n == 5, f"examined {n} sealing steps, expected 5"
PY
[ "$priv" -eq 1 ] && ok "the five sealing steps examined never print a log or enable tracing" || bad "a new step prints a log or traces"

echo "fuzz-seal-finding: $pass passed, $fail failed (expected $EXPECTED_CASES cases)"
[ "$fail" -eq 0 ] && [ "$pass" -eq "$EXPECTED_CASES" ]
