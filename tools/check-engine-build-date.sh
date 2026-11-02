#!/usr/bin/env bash
# The built engine artifacts carry no build date except the pinned one.
#
# WHY (owner (95)(1), 2026-11-02). libjpeg-turbo stamps "libjpeg-turbo version 3.2.0 (build
# YYYYMMDD)" into libjpeg.a from CMake's `string(TIMESTAMP)`, and that string is linked into
# qpdf.wasm. Rebuilding the wasm engines on another day changed 3 bytes of qpdf.wasm and moved
# its brotli size by 234-387 B, which looked like a size regression. engines/build-{native,wasm}.sh
# now pass -DBUILD=<pinned> and export SOURCE_DATE_EPOCH, both derived from pins.toml
# [libjpeg-turbo] build_date. This check reads the ARTIFACTS, so a build script that drops the
# flag, or a new dependency that stamps __DATE__, is caught on the bytes that ship.
#
# RULES (each has a probe run on every invocation: a positive fixture it must match and a
# near-miss it must not -- a rule that matches nothing passes everything):
#   build-stamp  every "(build YYYYMMDD)" equals the pinned date's YYYYMMDD;
#   date-macro   no `__DATE__` shape (Mmm dd yyyy, as in "Nov  2 2026");
#   built-on     no "built on" / "compiled on" phrase.
# Not flagged, because they are constants and not build dates: PDF-spec example dates, a
# "%%CreationDate: Tue Oct 31 01:34:22 2006" inside PDFium, the rustc release date in the
# compiler version string. The rules are shaped so those do not match (the probes say so).
#
# WHAT IT EXAMINED is printed by name. Required: qpdf.wasm, pdfium.wasm and the wasm libjpeg.a,
# and the pinned stamp must be SEEN in qpdf.wasm and in libjpeg.a (a rule that finds no stamp
# examined nothing). The three burrow_wasm_bg.wasm bindings and a native libjpeg.a are examined
# when present; --require-bindings makes the bindings required (the build-web-payload action,
# which has just built them).
#
# NOT SCANNED, by design of what exists today: the native libqpdf.a, libpdfium.so and libz.a, and
# the emscripten .js glue (qpdf.js, pdfium.js, bin/*.js). The wasm artifacts are what ships to the
# browser; the native ones are test and fuzz inputs. There is no `__TIME__` (hh:mm:ss) rule and no
# ISO-date rule, so a future dependency that stamps either passes. Its own `grep` found none
# (rounds/repro-engine-date/grep-results.txt); add a rule and its probe if one appears.
#
# Usage: tools/check-engine-build-date.sh [--require-bindings] [--pinned YYYYMMDD]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
require_bindings=0
pinned=""
while [ $# -gt 0 ]; do
  case "$1" in
    --require-bindings) require_bindings=1 ;;
    --pinned) pinned="$2"; shift ;;
    *) echo "check-engine-build-date: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done

if [ -z "$pinned" ]; then
  # shellcheck source=engines/pins.sh
  source "$repo/engines/pins.sh"
  bd="$(pins_get libjpeg-turbo.build_date)" || { echo "check-engine-build-date: no libjpeg-turbo.build_date in pins.toml" >&2; exit 1; }
  pinned="$(date -u -d "$bd" +%Y%m%d)" || { echo "check-engine-build-date: bad build_date $bd" >&2; exit 1; }
fi

exec python3 -I - "$repo" "$pinned" "$require_bindings" <<'PY'
import re, sys, pathlib

repo, pinned, require_bindings = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3] == "1"
MONTHS = rb"(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)"
STAMP = re.compile(rb"\(build (\d{8})\)")
DATE_MACRO = re.compile(rb"(?<![A-Za-z0-9])" + MONTHS + rb" [ 0-9][0-9] 20[0-9]{2}(?![0-9])")
BUILT_ON = re.compile(rb"(?i)\b(?:built|compiled) on\b")

# (rule, regex, positive fixture it must match, near-miss it must not)
PROBES = [
    ("build-stamp", STAMP, b"libjpeg-turbo version 3.2.0 (build 20991231)", b"3.2.0 build 2099 (12)"),
    ("date-macro", DATE_MACRO, b"x Nov  2 2026 y", b"%%CreationDate: Tue Oct 31 01:34:22 2006"),
    ("built-on", BUILT_ON, b"this was built on a laptop", b"rebuilt only once; built-in table"),
]
for rule, rx, pos, near in PROBES:
    if not rx.search(pos):
        sys.exit(f"check-engine-build-date: REFUSED -- probe: rule '{rule}' does not match its own positive fixture")
    if rx.search(near):
        sys.exit(f"check-engine-build-date: REFUSED -- probe: rule '{rule}' matches its near-miss")
print(f"probes: {len(PROBES)} rules, each matches its positive fixture and rejects its near-miss")

vendor = repo / "engines/vendor"
required = [vendor / "wasm/lib/qpdf.wasm", vendor / "wasm/lib/pdfium.wasm", vendor / "wasm/lib/libjpeg.a"]
bindings = [repo / f"bindings/burrow-wasm/{d}/burrow_wasm_bg.wasm" for d in ("pkg", "pkg-render", "pkg-redact")]
optional = sorted(vendor.glob("native-*/lib/libjpeg.a")) + bindings
if require_bindings:
    required += bindings
    optional = [p for p in optional if p not in bindings]

missing = [str(p.relative_to(repo)) for p in required if not p.is_file()]
if missing:
    sys.exit("check-engine-build-date: REFUSED -- required artifact(s) missing: " + ", ".join(missing))
examined = required + [p for p in optional if p.is_file()]
skipped = [str(p.relative_to(repo)) for p in optional if not p.is_file()]

bad, stamps_seen = [], {}
for p in examined:
    data = p.read_bytes()
    name = str(p.relative_to(repo))
    for m in STAMP.finditer(data):
        stamps_seen.setdefault(name, []).append(m.group(1).decode())
        if m.group(1).decode() != pinned:
            bad.append(f"{name}: build-stamp (build {m.group(1).decode()}), pinned is {pinned}")
    for rule, rx in (("date-macro", DATE_MACRO), ("built-on", BUILT_ON)):
        for m in rx.finditer(data):
            bad.append(f"{name}: {rule}: {m.group(0)!r}")
for must in ("engines/vendor/wasm/lib/qpdf.wasm", "engines/vendor/wasm/lib/libjpeg.a"):
    if not stamps_seen.get(must):
        bad.append(f"{must}: no libjpeg-turbo build stamp seen -- the rule examined nothing there")

print(f"examined {len(examined)} artifact(s): " + ", ".join(str(p.relative_to(repo)) for p in examined))
if skipped:
    print(f"not present, not examined: " + ", ".join(skipped))
print("stamps seen: " + "; ".join(f"{k}={v}" for k, v in sorted(stamps_seen.items())) + f" (pinned {pinned})")
if bad:
    print("check-engine-build-date: REFUSED --", file=sys.stderr)
    for b in bad:
        print("  " + b, file=sys.stderr)
    sys.exit(1)
print("check-engine-build-date: OK")
PY
