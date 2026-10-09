#!/usr/bin/env bash
# third_party/adobe-core14-afm/ is Adobe's Core 14 AFM archive, byte for byte (ADR 0030, #290).
#
# The notice those files carry permits use and redistribution on three conditions: every copyright
# notice kept, the AFM files never distributed without MustRead.html, and any modification noted.
# This check is how the repository keeps the first two and proves the third does not arise:
#
#   - the directory holds EXACTLY the files third_party/adobe-core14-afm.toml records -- no fewer
#     (an AFM shipped without its notice, or a notice without its files), and no more;
#   - each file's sha256 is the recorded one, so nothing in it has been edited;
#   - with --fetch, the archive is re-downloaded from the pinned URL, checked against the recorded
#     archive checksum, and every file in it compared byte for byte with the committed copy.
#
# It reports how many files it examined against how many the record names.
#
# Usage: tools/check-afm-provenance.sh [--fetch]
# AFM_DIR and AFM_RECORD override the two paths (self-test only).
# Self-test: tools/test-check-afm-provenance.sh.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
dir="${AFM_DIR:-$repo/third_party/adobe-core14-afm}"
record="${AFM_RECORD:-$repo/third_party/adobe-core14-afm.toml}"
fetch=0
[ "${1:-}" = "--fetch" ] && fetch=1

python3 -I - "$dir" "$record" "$fetch" <<'PY'
import hashlib, io, pathlib, sys, tomllib, urllib.request, zipfile

directory, record_path, fetch = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3] == "1"
problems = []
record = tomllib.loads(record_path.read_text(encoding="utf-8"))
expected = record.get("files", {})
notice = record.get("archive", {}).get("notice")
if not expected:
    problems.append(f"{record_path} records no files; a check of nothing is not a check")
if notice not in expected:
    problems.append(f"the notice {notice!r} is not among the recorded files, so the AFMs could ship without it")

present = sorted(p.name for p in directory.iterdir()) if directory.is_dir() else []
for name in sorted(set(expected) - set(present)):
    problems.append(f"{name} is recorded and missing from {directory}")
for name in sorted(set(present) - set(expected)):
    problems.append(f"{name} is in {directory} and not in the record -- an addition to Adobe's archive")
examined = 0
for name in sorted(set(present) & set(expected)):
    examined += 1
    digest = hashlib.sha256((directory / name).read_bytes()).hexdigest()
    if digest != expected[name]:
        problems.append(f"{name} differs from the published file (sha256 {digest[:16]}..., recorded {expected[name][:16]}...) -- the notice requires a modification to be marked, and this one is not")

fetched = "not fetched"
if fetch and not problems:
    archive = record["archive"]
    with urllib.request.urlopen(archive["url"], timeout=60) as response:
        data = response.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != archive["sha256"]:
        problems.append(f"the archive at {archive['url']} is not the pinned one (sha256 {digest[:16]}...)")
    else:
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            names = sorted(z.namelist())
            if names != sorted(expected):
                problems.append(f"the archive holds {names}, not the recorded set")
            for name in names:
                if name in expected and z.read(name) != (directory / name).read_bytes():
                    problems.append(f"{name} in the archive differs from the committed copy")
        fetched = "archive fetched and matched byte for byte"

print(f"check-afm-provenance: {examined} of {len(expected)} recorded file(s) examined in {directory.name}; {fetched}")
if problems:
    print(f"FAILED -- {len(problems)} problem(s):", file=sys.stderr)
    for p in problems:
        print(f"  - {p}", file=sys.stderr)
    sys.exit(1)
print("OK -- Adobe's Core 14 AFM files, as published, with their notice.")
PY
