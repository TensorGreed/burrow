#!/usr/bin/env bash
# PostToolUse hook: format ONLY the file that was just edited.
#
# This must stay fast — it runs after every Edit and Write. So: one file, one formatter,
# no workspace-wide passes, no linting, no builds.
#
# Exit codes, and why:
#
#   0  formatted, or nothing to do. The overwhelmingly common case.
#   1  the formatter this file needs is missing. A non-blocking error: the edit still
#      stands, but the message reaches the user instead of vanishing. Silence here is
#      how rustfmt went un-run for an entire session — the hook reported success while
#      formatting nothing.
#
# Never exit 2, and never fail because a formatter *ran* and disliked the file. A
# formatting problem must not block an edit; only a missing tool is worth reporting, and
# only because it is invisible otherwise.
#
# Claude Code passes the tool payload as JSON on stdin.
set -uo pipefail

# Hook shells do not inherit an interactive profile, so cargo's bin directory is
# usually absent from PATH.
PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"

payload=$(cat 2>/dev/null || true)

# Pull the edited path out of the payload. Prefer jq; fall back to a narrow grep so the
# hook still works on a machine without jq.
if command -v jq >/dev/null 2>&1; then
  file=$(printf '%s' "$payload" | jq -r '.tool_input.file_path // empty' 2>/dev/null)
else
  file=$(printf '%s' "$payload" | grep -o '"file_path"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed 's/.*:[[:space:]]*"//; s/"$//')
fi

[ -n "${file:-}" ] && [ -f "$file" ] || exit 0

repo_root=$(git -C "$(dirname "$file")" rev-parse --show-toplevel 2>/dev/null) || exit 0

case "$file" in
  *.rs)
    # rustfmt directly on the single file: `cargo fmt` would format the whole workspace.
    if ! command -v rustfmt >/dev/null 2>&1; then
      echo "format-file.sh: rustfmt not found, so $file was left unformatted." >&2
      echo "  Install it with: rustup component add rustfmt" >&2
      echo "  (CI runs 'cargo fmt --all -- --check' and will fail on this.)" >&2
      exit 1
    fi
    rustfmt --edition 2024 "$file" >/dev/null 2>&1
    ;;
  *.astro | *.svelte | *.ts | *.tsx | *.js | *.mjs | *.cjs | *.json | *.jsonc | *.css | *.scss | *.html | *.md | *.yml | *.yaml)
    # Only inside apps/web. That is exactly what `pnpm lint` checks in CI, and the
    # prettier config lives there. Running it repo-wide reformats files nothing
    # verifies -- padding every markdown table and rewriting emphasis markers -- which
    # buries a one-line change in twenty lines of churn.
    web="$repo_root/apps/web"
    case "$file" in
      "$web"/* | apps/web/*) ;;
      *) exit 0 ;;
    esac
    if [ ! -x "$web/node_modules/.bin/prettier" ]; then
      echo "format-file.sh: prettier not found, so $file was left unformatted." >&2
      echo "  Install it with: pnpm -C apps/web install" >&2
      echo "  (CI runs 'pnpm lint' and will fail on this.)" >&2
      exit 1
    fi
    (cd "$web" && ./node_modules/.bin/prettier --write --ignore-unknown "$file" >/dev/null 2>&1)
    ;;
esac

exit 0
