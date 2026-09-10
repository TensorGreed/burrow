# Read values out of engines/pins.toml. Source this; do not execute it.
#
# WHY: version strings were hardcoded fifteen times across the build scripts and asserted
# again in two tests. A bump in pins.toml would then leave the scripts fetching a file
# that exists (fetch.sh only checks what pins.toml names) while building a version nobody
# asked for. pins.toml is the source of truth or it is decoration.
#
# python3 is already a hard requirement of fetch.sh, so this adds no dependency.

# pins_get <dotted.key> -- e.g. pins_get qpdf.version
pins_get() {
  local key="$1"
  local pins="${BURROW_PINS:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pins.toml}"
  local value
  value="$(python3 - "$pins" "$key" <<'PY'
import sys, tomllib

with open(sys.argv[1], "rb") as fh:
    node = tomllib.load(fh)
for part in sys.argv[2].split("."):
    if not isinstance(node, dict) or part not in node:
        sys.exit(f"pins.toml: no such key: {sys.argv[2]}")
    node = node[part]
if not isinstance(node, (str, int, float)):
    sys.exit(f"pins.toml: {sys.argv[2]} is not a scalar")
print(node)
PY
)" || return 1
  [ -n "$value" ] || return 1
  printf '%s' "$value"
}
