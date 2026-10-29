#!/usr/bin/env bash
# Build the two files the browser differential takes from the native side, and nothing else.
#
#   tests/conformance/native-outcomes.json   written by core/burrow-ops/tests/conformance.rs
#   tests/redaction/generated/*.pdf          built by tools/check-redaction-corpus.sh's generators
#
# WHY (owner (83)(1), 2026-10-29). CI's web jobs used to `needs: [test]` for these two files, and
# `test` is the longest job in the workflow. This is the short job that builds them instead, so the
# web jobs start with `test` rather than after it. `test` still builds both itself, because its own
# suites read them.
#
# NEEDS THE VENDORED NATIVE ENGINES, and that is load-bearing (CLAUDE.md, where a check lives): the
# conformance test links the native qpdf and PDFium, and the generators use the vendored `cjpeg`
# and `qpdf`. Without `engines/vendor` the test does not build and the corpus check refuses, so a
# job that fetched nothing would go red here rather than hand over fewer fixtures.
#
# `-p burrow-ops --all-features`, the crate and feature set `test`'s `cargo test --workspace
# --all-features` builds the record with. The PR that introduced this showed, from its own CI run,
# that both files are byte-identical to `test`'s (owner (83)(1); see the PR body).
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

record="tests/conformance/native-outcomes.json"
rm -f "$record"

"$root/tools/check-redaction-corpus.sh"
cargo test -p burrow-ops --all-features --test conformance

[ -s "$record" ] || { echo "::error::build-native-handover: $record was not written" >&2; exit 1; }
count="$(find tests/redaction/generated -maxdepth 1 -name '*.pdf' | wc -l | tr -d ' ')"
echo "build-native-handover: $record ($(wc -c < "$record" | tr -d ' ') bytes) and $count generated document(s)"
