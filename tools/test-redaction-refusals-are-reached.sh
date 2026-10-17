#!/usr/bin/env bash
# Mutation test: each of #125's five refusals, and the routes its evasion fixtures take, is
# REACHED -- delete the check and a named fixture must be freed by it.
#
# #125 asks every refusal to fire on a fixture built to evade its signal, and every such fixture to
# have a near-miss twin. A fixture and a twin show the refusal fires today; they do not show the
# fixture is what would notice the refusal going. Until slice 4 that was recorded only in PR bodies,
# where nothing re-runs it. This plants each deletion in the tree, runs the suite that pins the
# fixtures by rule, and requires it to go red NAMING the fixtures the deletion frees:
#
#   optional-content      the redaction walk's refusal       evade-oc-inside-an-unused-type3-font
#   split-oc-form-route   split's walk of a form's /Resources  layer-at-depth-2 .. 8
#   split-oc-type3-route  split's walk of a font's /Resources  layer-in-type3-glyph
#   image-in-region       an image `Do`'s ink box             evade-image-in-form, ...-two-levels-down
#   vector-in-region      a fill's ink box                     evade-paths-in-form, ...-two-levels-down
#   shading-in-region     `sh`'s ink box                       evade-shading-sh-in-form
#   form-route            the ink a nested form draws          the four form evasions, and the shading
#   acroform-field-ft     the /FT scan                         evade-widget-ft-on-parent, ...
#   acroform-catalogue    the catalogue /AcroForm key          the direct-/Root catalogue test
#   structure-tree        the whole structure-tree refusal     evade-struct-without-structparents, its twin
#
# EACH MUTATION IS ASSERTED TWICE BEFORE ITS RESULT COUNTS. That it applied: the anchor occurs
# exactly once and the file changed -- a replace that matched nothing is indistinguishable from a
# defence that holds. And that cargo REBUILT `burrow-engines` for it: cargo decides by mtime, and a
# red read off a binary built before the plant is the previous mutation's result, not this one's.
#
# The plants compile warning-free under CI's `-D warnings`: a deletion is a branch on
# `std::hint::black_box`, which the compiler cannot fold, so nothing becomes unreachable or unused.
# Red for a compile error is red for the wrong reason, and `require_red` would refuse it -- it
# matches on the fixture names, never on the exit status alone.
#
# Usage: tools/test-redaction-refusals-are-reached.sh [mutation ...]   (default: all of them)

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

files=(
  core/burrow-engines/src/redact/optional_content.rs
  core/burrow-engines/src/prune/mod.rs
  core/burrow-engines/src/pdfsyntax/geometry.rs
  core/burrow-engines/src/redact/steps.rs
)
marker='MUTANT(test-redaction-refusals-are-reached)'

# REFUSE IF A PREVIOUS RUN'S MUTATION IS STILL IN THE TREE. The EXIT trap below survives INT and
# TERM; a SIGKILL or an OOM kill does not, and what it leaves COMPILES and refuses less. So the
# residue is looked for by the marker every plant carries, which nothing else writes.
for f in "${files[@]}"; do
  if grep -qF "$marker" "$f"; then
    echo "REFUSED -- $f still carries a planted mutation ($marker) from an earlier run." >&2
    echo "  That tree compiles and refuses less. Recover with: git checkout -- $f" >&2
    exit 1
  fi
done

if [ ! -d "$repo/engines/vendor" ]; then
  echo "SKIP  every mutation: engines/vendor is absent (engines/fetch.sh && engines/build-native.sh)"
  echo "      Nothing was measured. This is not a pass."
  exit 0
fi

backup="$(mktemp -d)"
for i in "${!files[@]}"; do cp "${files[$i]}" "$backup/$i"; done
restore() {
  for i in "${!files[@]}"; do cp "$backup/$i" "${files[$i]}"; done
}
trap 'restore; rm -rf "$backup"' EXIT

# The mutations: file, the text it must find EXACTLY ONCE, and what replaces it.
plant() {
  python3 - "$1" "$marker" <<'PY'
import sys

name, marker = sys.argv[1], sys.argv[2]
off = f"std::hint::black_box(true) /* {marker} */"
MUTATIONS = {
    "optional-content": (
        "core/burrow-engines/src/redact/optional_content.rs",
        ") -> Result<()> {\n    let mut walk = Walk {",
        ") -> Result<()> {\n    if " + off + " {\n        return Ok(());\n    }\n    let mut walk = Walk {",
    ),
    "split-oc-form-route": (
        "core/burrow-engines/src/prune/mod.rs",
        "                refuse_optional_content_in(graph, &own)?;\n                follow_resources(",
        "                if !" + off + " {\n                    refuse_optional_content_in(graph, &own)?;\n"
        "                }\n                follow_resources(",
    ),
    "split-oc-type3-route": (
        "core/burrow-engines/src/prune/mod.rs",
        "            let own = graph.key(object, RESOURCES_KEY)?;\n"
        "            if graph.type_code(&own)? == object_type::DICTIONARY {\n"
        "                refuse_optional_content_in(graph, &own)?;\n",
        "            let own = graph.key(object, RESOURCES_KEY)?;\n"
        "            if graph.type_code(&own)? == object_type::DICTIONARY && !" + off + " {\n"
        "                refuse_optional_content_in(graph, &own)?;\n",
    ),
    "image-in-region": (
        "core/burrow-engines/src/pdfsyntax/geometry.rs",
        "        push_finite_ink(ink, InkKind::Image, UNIT_SQUARE.transformed(&state.ctm));\n"
        "        return Ok(());",
        "        if !" + off + " {\n"
        "            push_finite_ink(ink, InkKind::Image, UNIT_SQUARE.transformed(&state.ctm));\n"
        "        }\n        return Ok(());",
    ),
    "vector-in-region": (
        "core/burrow-engines/src/pdfsyntax/geometry.rs",
        "                        push_finite_ink(ink, InkKind::Vector, box_.transformed(&state.ctm));",
        "                        if !" + off + " {\n"
        "                            push_finite_ink(ink, InkKind::Vector, box_.transformed(&state.ctm));\n"
        "                        }",
    ),
    "shading-in-region": (
        "core/burrow-engines/src/pdfsyntax/geometry.rs",
        'b"sh" => ink.push(InkBox {\n                kind: InkKind::Shading,\n                extent: None,\n            }),',
        'b"sh" => {\n                if !' + off + " {\n                    ink.push(InkBox {\n"
        "                        kind: InkKind::Shading,\n                        extent: None,\n"
        "                    });\n                }\n            }",
    ),
    "form-route": (
        "core/burrow-engines/src/pdfsyntax/geometry.rs",
        "    let result = walk(\n        &form.content,\n        inner,\n        GraphicsState {\n"
        "            ctm: form.matrix.then(&state.ctm),\n            ..state.clone()\n        },\n"
        "        budget,\n        out,\n        ink,\n    );",
        "    let mut dropped = Vec::new();\n    let result = walk(\n        &form.content,\n        inner,\n"
        "        GraphicsState {\n            ctm: form.matrix.then(&state.ctm),\n"
        "            ..state.clone()\n        },\n        budget,\n        out,\n"
        "        if " + off + " { &mut dropped } else { ink },\n    );",
    ),
    "acroform-field-ft": (
        "core/burrow-engines/src/redact/steps.rs",
        "            if is_field {\n                return Err(refusal());",
        "            if is_field && !" + off + " {\n                return Err(refusal());",
    ),
    "acroform-catalogue": (
        "core/burrow-engines/src/redact/steps.rs",
        "        if has_acro_form {\n            return Err(refusal());",
        "        if has_acro_form && !" + off + " {\n            return Err(refusal());",
    ),
    "structure-tree": (
        "core/burrow-engines/src/redact/steps.rs",
        "    pub(crate) fn refuse_structure_tree(&self) -> Result<()> {\n",
        "    pub(crate) fn refuse_structure_tree(&self) -> Result<()> {\n"
        "        if " + off + " {\n            return Ok(());\n        }\n",
    ),
}
path, old, new = MUTATIONS[name]
source = open(path, encoding="utf-8").read()
count = source.count(old)
assert count == 1, f"{name}: the anchor occurs {count} times in {path}, not once"
mutated = source.replace(old, new)
assert mutated != source and marker in mutated, f"{name}: the mutation did not apply"
open(path, "w", encoding="utf-8").write(mutated)
print(f"planted {name} in {path}")
PY
}

pass=0
fail=0

# Run a suite with `name` planted and require it to FAIL, after REBUILDING burrow-engines, with
# every one of the expected texts in its output.
require_red() {
  local name="$1" expects="$2"
  shift 2
  plant "$name"
  local out status
  set +e
  out="$("$@" 2>&1)"
  status=$?
  set -e
  restore
  if ! grep -qE '^\s*Compiling burrow-engines ' <<<"$out"; then
    echo "FAIL  $name: cargo did not rebuild burrow-engines, so the suite ran a binary built before the plant"
    fail=$((fail + 1))
    return
  fi
  if [ "$status" -eq 0 ]; then
    echo "FAIL  $name: the suite passed with the check deleted"
    fail=$((fail + 1))
    return
  fi
  local missing=() expect
  IFS='|' read -r -a wanted <<<"$expects"
  for expect in "${wanted[@]}"; do
    grep -qF "$expect" <<<"$out" || missing+=("$expect")
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    echo "FAIL  $name: red, but the output does not name: ${missing[*]}"
    echo "$out" | tail -40
    fail=$((fail + 1))
    return
  fi
  echo "PASS  $name: rebuilt, red, naming ${#wanted[@]} expected: $expects"
  pass=$((pass + 1))
}

defences=(cargo test -p burrow-engines --features native-engines --test redaction_defences)
resolved="${defences[*]} a_named_property_list_and_a_layer_refuse_for_their_own_reason"
ink="${defences[*]} an_image_a_shading_or_a_path_refuses_by_its_own_rule_through_every_route"
split=(cargo test -p burrow-ops --features native-engines --test split_no_leak)

run() {
  case "$1" in
    optional-content)
      require_red "$1" \
        "evade-oc-inside-an-unused-type3-font.pdf: redacted|evade-oc-on-an-annotation.pdf: redacted" \
        $resolved ;;
    split-oc-form-route)
      require_red "$1" "a layer 2 forms down: not refused by the per-page check" \
        "${split[@]}" a_layer_at_every_depth_the_walk_follows_is_refused ;;
    split-oc-type3-route)
      require_red "$1" "a layer in a Type 3 glyph: not refused by the per-page check" \
        "${split[@]}" a_layer_inside_a_type_three_glyph_procedure_is_refused ;;
    image-in-region)
      require_red "$1" \
        "evade-image-in-form.pdf: redacted|evade-image-in-form-two-levels-down.pdf: redacted" \
        $ink ;;
    vector-in-region)
      require_red "$1" \
        "evade-paths-in-form.pdf: redacted|evade-paths-in-form-two-levels-down.pdf: redacted" \
        $ink ;;
    shading-in-region)
      require_red "$1" "evade-shading-sh-in-form.pdf: redacted" $ink ;;
    form-route)
      require_red "$1" \
        "evade-image-in-form.pdf: redacted|evade-image-in-form-two-levels-down.pdf: redacted|evade-paths-in-form.pdf: redacted|evade-paths-in-form-two-levels-down.pdf: redacted|evade-shading-sh-in-form.pdf: redacted" \
        $ink ;;
    acroform-field-ft)
      # NOT "redacted": the suite's band reaches each widget's /Rect, so with the /FT scan gone
      # both fall to `[annotation-in-region]` -- moved off their rule, and named for it.
      require_red "$1" \
        "evade-widget-ft-on-parent.pdf: refused, but not by \`acroform-field\`|evade-field-without-acroform.pdf: refused, but not by \`acroform-field\`" \
        $resolved ;;
    acroform-catalogue)
      require_red "$1" "a_direct_root_catalogue_carrying_a_form_is_refused" \
        "${defences[@]}" a_direct_root_catalogue_carrying_a_form_is_refused ;;
    structure-tree)
      # The golden: the structure-tree fixtures are pinned there, region by region.
      require_red "$1" \
        "evade-struct-without-structparents.pdf|nearmiss-structparents-but-nothing-in-region.pdf" \
        cargo test -p burrow-ops --features native-engines --test redaction_outcomes ;;
    *)
      echo "unknown mutation: $1" >&2
      exit 2 ;;
  esac
}

all=(optional-content split-oc-form-route split-oc-type3-route image-in-region vector-in-region
  shading-in-region form-route acroform-field-ft acroform-catalogue structure-tree)
selected=("$@")
[ "${#selected[@]}" -gt 0 ] || selected=("${all[@]}")
for name in "${selected[@]}"; do run "$name"; done

echo
echo "------------------------------------------------------------"
echo "$pass passed, $fail failed, of ${#selected[@]} mutation(s) (${#all[@]} defined)"
[ "$fail" -eq 0 ] && [ "$pass" -eq "${#selected[@]}" ]
