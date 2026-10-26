#!/usr/bin/env bash
# Seal a fuzz finding: the crash inputs and the UNREDACTED log, encrypted with age to the owner's
# public key, so they can be kept without being published.
#
#   tools/fuzz-seal-finding.sh <target> <unredacted-log> <recipient-file> <out-dir> [artifacts-dir]
#
# Writes <out-dir>/<target>.age and nothing else there. The workflow uploads ONLY that file.
#
# WHY (owner decision (62)(1), 2026-10-24). Nightly 40076578189 found a new `reorder` crash. The
# public log is redacted (tools/redact-fuzz-log.py) so the internal-error message was lost, and the
# input was never kept, because the nightly had stopped uploading artifacts: on a public repository
# that published reproducers (see "THE SIGNAL, NEVER THE PAYLOAD" in fuzz-nightly.yml). The input
# and the message may now be kept, but only ciphertext leaves the runner; the private key is
# owner-held and is not in this repository or in CI.
#
# EXIT CODES (the workflow reads 4 only):
#   0  sealed; <out-dir>/<target>.age exists
#   4  the recipient file still holds the PLACEHOLDER: nothing was sealed or written. Not a
#      failure of the tool -- the owner has not installed a key yet.
#   1  anything else. FAIL CLOSED: on any failure no file is left in <out-dir>.
#
# PLAINTEXT NEVER REACHES <out-dir>. The bundle is staged in a private (0700) mktemp directory that
# is outside <out-dir>, deleted on exit; it is written to disk there because a pipe would put tar's
# status on the left of a pipe. On a CI runner that directory dies with the VM. Nothing in this
# script prints the log or any input byte.
#
# Env: AGE (default `age`) is the binary. It must report exactly the version pinned in
# tools/install-age.sh. GITHUB_OUTPUT, if set, receives `sealed=true|false`.
#
# Self-test: tools/test-fuzz-seal-finding.sh.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
target="${1:?usage: fuzz-seal-finding.sh <target> <log> <recipient-file> <out-dir> [artifacts-dir]}"
log="${2:?missing log}"
recipient_file="${3:?missing recipient file}"
out_dir="${4:?missing out dir}"
artifacts="${5:-fuzz/artifacts/$target}"
age="${AGE:-age}"
: "${GITHUB_OUTPUT:=/dev/null}"

out="$out_dir/$target.age"
stage=""
status_note() { echo "fuzz-seal-finding: $*" >&2; }

cleanup() {
  local rc=$?
  [ -n "$stage" ] && rm -rf "$stage"
  # FAIL CLOSED: a failed run leaves nothing to upload.
  if [ "$rc" -ne 0 ]; then
    rm -f "$out" "$out.partial"
    echo "sealed=false" >> "$GITHUB_OUTPUT"
  fi
  return 0
}
trap cleanup EXIT

# The target name becomes a file name and a tar member; refuse anything that is not a plain name.
case "$target" in
  '' | *[!A-Za-z0-9_]*) status_note "refusing target name '$target'"; exit 1 ;;
esac

# THE RECIPIENT: comments and blank lines dropped, exactly one line left, and it must be an age
# X25519 public key (age1 + 58 bech32 characters). The placeholder is recognised by name and is
# the only thing that exits 4; a malformed key is a failure, not a skip.
[ -f "$recipient_file" ] || { status_note "no recipient file at $recipient_file"; exit 1; }
recipient_lines="$(grep -vE '^[[:space:]]*(#|$)' "$recipient_file" || true)"
if [ "$(printf '%s\n' "$recipient_lines" | grep -c .)" -ne 1 ]; then
  status_note "$recipient_file must hold exactly one recipient line"
  exit 1
fi
recipient="$(printf '%s' "$recipient_lines" | tr -d '[:space:]')"
case "$recipient" in
  PLACEHOLDER*)
    status_note "$recipient_file is still the placeholder; nothing sealed, nothing uploaded"
    echo "sealed=false" >> "$GITHUB_OUTPUT"
    trap - EXIT
    exit 4
    ;;
esac
if ! printf '%s' "$recipient" | grep -qE '^age1[a-z0-9]{58}$'; then
  status_note "$recipient_file does not hold an age X25519 public key (age1 + 58 characters)"
  exit 1
fi

# THE PINNED age, by version. install-age.sh pins the release asset by sha256; this refuses a
# different binary that happens to be on PATH.
want_version="$("$here/install-age.sh" --version)"
got_version="$("$age" --version)"
if [ "$got_version" != "$want_version" ]; then
  status_note "age is $got_version, not the pinned $want_version"
  exit 1
fi

[ -s "$log" ] || { status_note "log $log is missing or empty; nothing to seal"; exit 1; }

stage="$(mktemp -d)"
chmod 700 "$stage"
mkdir "$stage/bundle"
cp -- "$log" "$stage/bundle/fuzz.log"
inputs=0
if [ -d "$artifacts" ]; then
  mkdir "$stage/bundle/artifacts"
  for f in "$artifacts"/*; do
    [ -f "$f" ] || continue
    cp -- "$f" "$stage/bundle/artifacts/"
    inputs=$((inputs + 1))
  done
fi
tar -cf "$stage/bundle.tar" -C "$stage" bundle

mkdir -p "$out_dir"
rm -f "$out" "$out.partial"
"$age" -r "$recipient" -o "$out.partial" "$stage/bundle.tar"

# What got written must be age ciphertext, and nothing else may sit in the upload directory.
header="$(head -c 21 "$out.partial")"
if [ "$header" != "age-encryption.org/v1" ]; then
  status_note "output does not start with an age header"
  exit 1
fi
mv "$out.partial" "$out"
listed="$(find "$out_dir" -mindepth 1 | sort)"
if [ "$listed" != "$out" ]; then
  status_note "upload directory holds more than $target.age"
  exit 1
fi

echo "sealed=true" >> "$GITHUB_OUTPUT"
status_note "sealed $inputs crash input(s) and the unredacted log ($(wc -c < "$log") bytes) -> $target.age ($(wc -c < "$out") bytes); age $got_version"
