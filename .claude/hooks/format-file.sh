#!/usr/bin/env bash
# PostToolUse hook: format ONLY the file that was just edited.
#
# This must stay fast — it runs after every Edit and Write. So: one file, one formatter,
# no workspace-wide passes, no linting, no builds. It exits 0 unconditionally; a
# formatting failure must never block an edit.
#
# Claude Code passes the tool payload as JSON on stdin.
set -uo pipefail

# Hook shells do not inherit an interactive profile, so cargo's bin directory is
# usually absent from PATH. Without this, rustfmt is silently not found and every
# .rs edit goes unformatted -- the failure is invisible because the hook always
# exits 0.
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
    command -v rustfmt >/dev/null 2>&1 && rustfmt --edition 2024 "$file" >/dev/null 2>&1
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
    if [ -x "$web/node_modules/.bin/prettier" ]; then
      (cd "$web" && ./node_modules/.bin/prettier --write --ignore-unknown "$file" >/dev/null 2>&1)
    fi
    ;;
esac

exit 0
