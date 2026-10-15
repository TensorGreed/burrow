#!/usr/bin/env bash
# Make a git worktree of this repository able to run `tools/ci-local.py --changed` on its own tree.
#
# WHY (2026-10-15, the owner's five-lane plan). `ci-local` refused in a fresh worktree --
# "engines-wasm: no stamp at engines/vendor/wasm/.build-stamp" -- because everything it measures
# against is gitignored and lives only in the main checkout: the engines, the corpus, the wasm
# bindings, `node_modules`. With one checkout able to sweep, five lanes queued for one slot.
#
# WHAT IT DOES, to the worktree named and nothing else:
#
#   engines/vendor     COPIED from the main checkout, never linked. A link would let a build in
#                      the worktree write into the main checkout's engines, and the main checkout's
#                      sweeps would measure it. A copy is 452 MB (2026-10-15) and cannot. Copied
#                      stamps are still checked against THIS tree's inputs: a branch whose engine
#                      pins differ refuses (`build-stamp.py check`), exactly as in the main checkout.
#   corpus/files       LINKED to the main checkout's, read-only by use: the corpus runner only reads
#                      it. Stated rather than enforced.
#   apps/web           `pnpm install --frozen-lockfile`, from this tree's lockfile.
#   bindings/pkg*      BUILT here (`ci-local.py --only wasm-pack`), so their stamps name this tree.
#   tests/redaction/generated   GENERATED here (`tools/check-redaction-corpus.sh`), because the
#                      census and the `outcomes.tsv` bless read it outside a sweep.
#   apps/web/dist      a PRODUCTION build (`pnpm build`), which `checker-self-tests` copies.
#   census harnesses   `~/burrow-census/zz_census.rs` and `zz_tiles.rs`, if present, copied into
#                      `core/burrow-engines/tests/` (gitignored). Never formatted with rustfmt:
#                      `rustfmt --edition 2021` follows `mod support;` into tracked files.
#
# Then it checks every stamp and says what it examined. Running it twice refreshes the bindings and
# leaves the copied engines alone unless `--refresh-engines` is given.
#
# Usage: tools/setup-worktree.sh <worktree> [--refresh-engines] [--no-build]
#   --no-build   copy and link only; skip pnpm and wasm-pack (the self-test uses this).
#
# Ports: Playwright's are fixed (4321, 4322). `ci-local.py` serialises Playwright jobs across every
# checkout with a machine-wide lock; a `pnpm e2e` started by hand does not take it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
usage() { echo "usage: tools/setup-worktree.sh <worktree> [--refresh-engines] [--no-build]" >&2; exit 2; }
[ $# -ge 1 ] || usage
target="$1"; shift
refresh=0; build=1
for arg in "$@"; do
  case "$arg" in
    --refresh-engines) refresh=1 ;;
    --no-build) build=0 ;;
    *) usage ;;
  esac
done

[ -d "$target" ] || { echo "setup-worktree: $target is not a directory" >&2; exit 2; }
target="$(cd "$target" && pwd -P)"

# THE MAIN CHECKOUT, from git rather than from where this script sits: the copy of this script
# being run may itself be in a worktree.
common="$(git -C "$target" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" || {
  echo "setup-worktree: $target is not inside a git repository" >&2; exit 2; }
main="$(cd "$common/.." && pwd -P)"
top="$(git -C "$target" rev-parse --show-toplevel)"
top="$(cd "$top" && pwd -P)"
if [ "$top" != "$target" ]; then
  echo "setup-worktree: $target is not the top of a worktree (its top is $top)" >&2; exit 2
fi
if [ "$target" = "$main" ]; then
  echo "setup-worktree: $target is the main checkout; it needs no setup and must not be overwritten" >&2
  exit 2
fi
[ -d "$main/engines/vendor" ] || {
  echo "setup-worktree: the main checkout has no engines/vendor to copy (run engines/fetch.sh and the builds there)" >&2
  exit 2; }

echo "setup-worktree: $target, from the main checkout $main"

# 1. ENGINES, copied.
if [ -L "$target/engines/vendor" ]; then
  echo "  engines/vendor is a LINK; replacing it with a copy, so nothing here can write into $main"
  rm "$target/engines/vendor"
fi
if [ -d "$target/engines/vendor" ] && [ "$refresh" -eq 0 ]; then
  echo "  engines/vendor: already a copy, kept (--refresh-engines to recopy)"
else
  rm -rf "$target/engines/vendor"
  cp -a "$main/engines/vendor" "$target/engines/vendor"
  echo "  engines/vendor: copied, $(find "$target/engines/vendor" -type f | wc -l) file(s)"
fi

# 2. CORPUS, linked.
if [ -d "$main/corpus/files" ]; then
  if [ ! -e "$target/corpus/files" ]; then
    ln -s "$main/corpus/files" "$target/corpus/files"
  fi
  echo "  corpus/files: $(readlink -f "$target/corpus/files")"
else
  echo "  corpus/files: the main checkout has none; the corpus job will say so"
fi

# 3. CENSUS HARNESSES, copied if present.
for harness in zz_census.rs zz_tiles.rs; do
  if [ -f "$HOME/burrow-census/$harness" ]; then
    cp "$HOME/burrow-census/$harness" "$target/core/burrow-engines/tests/$harness"
    echo "  census harness: $harness"
  fi
done

if [ "$build" -eq 1 ]; then
  # 4. NODE DEPENDENCIES and 5. THE BINDINGS, built in this tree.
  (cd "$target/apps/web" && pnpm install --frozen-lockfile --silent)
  echo "  apps/web: pnpm install done"
  python3 -B "$target/tools/ci-local.py" --only wasm-pack
  # 6. THE GENERATED REDACTION CORPUS, `tests/redaction/generated/` (gitignored). A sweep's
  # `redaction-corpus` job regenerates it, but the census and the `outcomes.tsv` bless read it
  # OUTSIDE a sweep -- and in a fresh worktree it did not exist: the bless failed on the missing
  # directory, and a census ran over the 127 inputs it could find and looked like a census.
  "$target/tools/check-redaction-corpus.sh" >/dev/null
  echo "  tests/redaction/generated: $(find "$target/tests/redaction/generated" -type f | wc -l) file(s)"
  # 7. A PRODUCTION BUILD in `apps/web/dist`. `checker-self-tests` copies one and refuses without
  # it ("no build at apps/web/dist ... refusing rather than passing vacuously"): measured, the one
  # job of 28 that failed in this script's first proof sweep.
  (cd "$target/apps/web" && pnpm -s build >/dev/null)
  echo "  apps/web/dist: a production build, $(find "$target/apps/web/dist" -type f | wc -l) file(s)"
fi

# THE CHECK, and what it examined. Without --no-build every stamp must be current; with it, only
# the engines' (the bindings are not built yet).
stamps="engines-wasm"
[ "$build" -eq 1 ] && stamps="engines-wasm pkg pkg-redact pkg-render"
(cd "$target" && python3 -B tools/build-stamp.py check $stamps)
echo "setup-worktree: OK -- $(wc -w <<<"$stamps") stamp(s) current in $target: $stamps"
