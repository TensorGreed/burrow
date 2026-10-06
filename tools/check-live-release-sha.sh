#!/usr/bin/env bash
# The live origin is serving THIS release's build -- read back from the deployed site.
#
#     tools/check-live-release-sha.sh <origin> <expected-sha>
#
# The build stamps the payload with `release-sha.txt` (the commit it was built from), and
# Cloudflare Pages serves it as text/plain. After a deploy, the live origin must return that file
# and it must equal the tagged sha. Run by the publish job after the upload, and by the
# OPERATIONS.md release command.
#
# DON'T TRUST THE STATUS CODE. A static host can answer a MISSING path with `200 OK` and an HTML
# body (a soft 404, a catch-all SPA rewrite). So this ignores the status and validates the BODY:
# it must be exactly a 40-char lowercase-hex sha with at most one trailing newline -- an HTML page
# fails that -- and it must equal the expected sha. `release-sha.txt` is a regular file (not a
# dotfile) precisely so Pages serves it reliably; this check is what proves it actually did.

set -uo pipefail

origin="${1:-}"
expected="${2:-}"

fail() { echo "refuse: $*" >&2; exit 1; }

[ -n "$origin" ] || fail "no origin given (arg 1)"
case "$expected" in
  *[!0-9a-f]* | "") fail "the expected sha '$expected' is not lowercase hex" ;;
esac
[ "${#expected}" -eq 40 ] || fail "the expected sha '$expected' is not 40 characters"

url="${origin%/}/release-sha.txt"

# Raw bytes, preserved exactly (the `printf x`/`%x` trick keeps a trailing newline that `$(…)`
# would strip). `-sS` without `-f`: a 200 soft-404 must reach the body check, not be waved through.
raw="$(curl -sS --max-time 30 "$url"; printf x)" || fail "could not fetch $url (network/TLS error)"
raw="${raw%x}"

# Strip at most ONE trailing newline, then the remainder must be exactly 40 hex. Two newlines, a
# space, or an HTML body all leave a non-hex byte in `body` and are refused.
body="$raw"
case "$body" in
  *$'\n') body="${body%$'\n'}" ;;
esac
case "$body" in
  "" | *[!0-9a-f]*)
    fail "$url did not return a bare 40-hex sha -- a missing path served as 200+HTML looks like this. First bytes: '$(printf '%.60s' "$raw")'" ;;
esac
[ "${#body}" -eq 40 ] || fail "$url returned ${#body} hex characters (after one optional newline), not 40"
[ "$body" = "$expected" ] || fail "$url is serving $body, not the expected $expected -- the live origin is not this release"

echo "OK -- live $url == $expected"
