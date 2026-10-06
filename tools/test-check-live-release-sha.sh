#!/usr/bin/env bash
# Adversarial self-test for tools/check-live-release-sha.sh.
#
# This tests the REAL thing, not a fixture: it stands up a local HTTP server that serves a chosen
# body with `200 OK` for any path, and runs the real curl-based check against it. The headline case
# is the one the live read exists to catch -- a host answering a missing path with `200` and an
# HTML body -- which must refuse. Needs curl and python3 (both present in CI and the runner).
#
# Every server it starts is killed in the same function that starts it, and a trap reaps any
# stray on exit, so this leaves no background process behind.

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$here/check-live-release-sha.sh"
SHA=0123456789abcdef0123456789abcdef01234567
pass=0
fail=0

work="$(mktemp -d)"
srv_pid=""
cleanup() {
  [ -n "$srv_pid" ] && kill "$srv_pid" 2>/dev/null
  rm -rf "$work"
}
trap cleanup EXIT

# A server that returns 200 + a fixed body (from a file) for EVERY path -- the soft-404 shape.
cat > "$work/serve.py" <<'PY'
import http.server, sys
body = open(sys.argv[1], "rb").read()
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a):
        pass
srv = http.server.HTTPServer(("127.0.0.1", 0), H)
# Tell the parent which port we got, then serve.
with open(sys.argv[2], "w") as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
PY

# serve_and_check <body-bytes-written-how> <want pass|fail> <label> <needle>
# The body is whatever is currently in $work/body.
serve_and_check() {
  local want=$1 label=$2 needle=$3
  rm -f "$work/port"
  python3 "$work/serve.py" "$work/body" "$work/port" >/dev/null 2>&1 &
  srv_pid=$!
  # Wait (bounded) for the port file; fail loudly rather than hang.
  local tries=0
  while [ ! -s "$work/port" ]; do
    tries=$((tries + 1))
    if [ "$tries" -gt 100 ] || ! kill -0 "$srv_pid" 2>/dev/null; then
      echo "  FAIL $label: the test server did not start"; fail=$((fail + 1))
      kill "$srv_pid" 2>/dev/null; srv_pid=""; return
    fi
    sleep 0.05
  done
  local port out status=0
  port="$(cat "$work/port")"
  out="$("$checker" "http://127.0.0.1:$port" "$SHA" 2>&1)" || status=$?
  kill "$srv_pid" 2>/dev/null; wait "$srv_pid" 2>/dev/null; srv_pid=""

  if [ "$want" = pass ]; then
    if [ "$status" -eq 0 ]; then echo "  ok   $label"; pass=$((pass + 1));
    else echo "  FAIL $label: expected pass, got $status"; sed 's/^/       /' <<<"$out"; fail=$((fail + 1)); fi
  else
    if [ "$status" -eq 0 ]; then echo "  FAIL $label: expected refusal, passed"; fail=$((fail + 1));
    elif grep -qF -- "$needle" <<<"$out"; then echo "  ok   $label"; pass=$((pass + 1));
    else echo "  FAIL $label: refused, but not for '$needle'"; sed 's/^/       /' <<<"$out"; fail=$((fail + 1)); fi
  fi
}

echo "check-live-release-sha self-test:"

# The match: bare 40-hex, no newline.
printf '%s' "$SHA" > "$work/body"
serve_and_check pass "the live sha served bare passes" ""

# A single trailing newline is tolerated (a server/CDN may add one).
printf '%s\n' "$SHA" > "$work/body"
serve_and_check pass "one trailing newline is tolerated" ""

# THE HEADLINE CASE: a missing path answered with 200 + an HTML body must refuse.
printf '<!doctype html><title>Not found</title>' > "$work/body"
serve_and_check fail "a 200 with an HTML body is refused" "did not return a bare 40-hex sha"

# A different (valid-looking) sha must refuse.
printf 'ffffffffffffffffffffffffffffffffffffffff' > "$work/body"
serve_and_check fail "a different sha is refused" "not this release"

# Two trailing newlines -> trailing byte beyond one newline -> refuse.
printf '%s\n\n' "$SHA" > "$work/body"
serve_and_check fail "more than one trailing newline is refused" "did not return a bare 40-hex sha"

# A truncated/short hex string must refuse.
printf '%s' "0123456789abcdef" > "$work/body"
serve_and_check fail "a short hex body is refused" "not 40"

# An empty body must refuse.
printf '' > "$work/body"
serve_and_check fail "an empty body is refused" "did not return a bare 40-hex sha"

if [ "$fail" -gt 0 ]; then
  echo "FAILED -- $fail case(s) failed, $pass passed"
  exit 1
fi
echo "OK -- $pass live-sha case(s) all behaved as required"
