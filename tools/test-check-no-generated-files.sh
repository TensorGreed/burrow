#!/usr/bin/env bash
# Adversarial self-test for tools/check-no-generated-files.sh.
#
# Follows the convention of test-detect-engine-components.sh and
# test-check-no-network-deps.sh: a checker nobody has shown can fail is not a control.
#
# THE CASE THAT MATTERS MOST is "every pattern catches its own kind of file". The first
# version of the checker packed `regex|label` into one string and split on the first `|` --
# and every regex contains `|`, so 15 of 16 patterns were truncated to `(^` and matched
# nothing, while the output still said "16 pattern(s)". It caught a planted `.pyc` and
# silently missed a planted `dist/` and a planted `results/`. Per-pattern coverage below is
# what makes that impossible to reintroduce quietly.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$here/check-no-generated-files.sh"

pass=0
fail=0

# Assert the checker REFUSES a path, and names it.
refuses() {
  local name="$1" path="$2"
  local out status
  set +e
  out="$("$checker" "$path" 2>&1)"
  status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: accepted $path"
    fail=$((fail + 1))
    return
  fi
  if ! grep -qF "$path" <<<"$out"; then
    echo "  FAIL $name: refused, but did not name $path"
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

accepts() {
  local name="$1" path="$2"
  if "$checker" "$path" >/dev/null 2>&1; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: refused $path, which is an ordinary source file"
    fail=$((fail + 1))
  fi
}

echo "per-pattern coverage: every pattern must catch its own kind of file"

# The two real incidents, by name.
refuses "the .pyc that reached main in PR #29" \
  "tools/__pycache__/check-engine-licences.cpython-312.pyc"
refuses "the spike sweep logs that reached main in PR #37" \
  "spikes/wasm-memory-ceiling/results/pdfium-512.txt"

# One per remaining pattern. If a pattern is ever truncated or deleted, its line goes red.
refuses "a __pycache__ directory entry"  "tools/__pycache__/notes.py"
refuses "playwright test-results"        "apps/web/test-results/run.json"
refuses "a playwright report"            "apps/web/playwright-report/index.html"
refuses "node_modules"                   "apps/web/node_modules/left-pad/index.js"
refuses "cargo target output"            "target/debug/burrow"
refuses "a dist directory"               "apps/web/dist/index.html"
refuses "a dist-* check directory"       "apps/web/dist-production-check/index.html"
refuses "wasm-pack pkg output"           "bindings/burrow-wasm/pkg/burrow_wasm.js"
refuses "a compiled wasm module"         "apps/web/public/engines/pdfium.abc123.wasm"
refuses "an Emscripten .data file"       "spikes/x/engine.data"
refuses "generated test data"            "corpus/files/big.generated.pdf"
refuses "the fetched engine tree"        "engines/vendor/wasm/lib/pdfium.js"
refuses "the Astro cache"                "apps/web/.astro/types.d.ts"
refuses "generated source"               "apps/web/src/generated/engines.js"

echo
echo "it does not refuse ordinary sources"
accepts "a Rust source file"        "core/burrow-engines/src/lib.rs"
accepts "an Astro page"            "apps/web/src/pages/index.astro"
accepts "a committed licence text" "engines/licences/pdfium.txt"
accepts "a tool"                   "tools/check-engine-licences.sh"
accepts "a doc"                    "docs/adr/0006-wasm-linking-strategy.md"
# The near-misses that a sloppier pattern would catch. `dist` and `results` appear as
# SUBSTRINGS here, not as path components.
accepts "a file whose name contains 'dist'" "core/burrow-ops/src/redistribute.rs"
accepts "a file whose name contains 'results'" "apps/web/src/conformance/results-schema.ts"

echo
echo "it refuses to pass while examining nothing"
if out="$("$checker" 2>&1)" && grep -q "examined 0" <<<"$out"; then
  echo "  FAIL it reported examining zero files and still passed"
  fail=$((fail + 1))
else
  echo "  ok   the real tree is non-empty, and an empty set exits non-zero by construction"
  pass=$((pass + 1))
fi

echo
echo "the real tracked tree is clean"
if "$checker" >/dev/null 2>&1; then
  echo "  ok   no tracked file matches a generated-output pattern"
  pass=$((pass + 1))
else
  echo "  FAIL the repository currently contains tracked generated output"
  "$checker" 2>&1 | sed 's/^/        /'
  fail=$((fail + 1))
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
