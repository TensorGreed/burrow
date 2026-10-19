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
#   split-oc-form-route   split's walk of a form's /Resources  layer-at-depth-2 (the loop stops there)
#   split-oc-type3-route  split's walk of a font's /Resources  layer-in-type3-glyph
#   image-in-region       an image `Do`'s ink box             evade-image-in-form, ...-two-levels-down
#   vector-in-region      a fill's ink box                     evade-paths-in-form, ...-two-levels-down
#   shading-in-region     `sh`'s ink box                       evade-shading-sh-in-form
#   form-route            the ink a nested form draws          the four form evasions, and the shading
#   acroform-field-ft     the /FT scan                         evade-widget-ft-on-parent, ...
#   acroform-catalogue    the catalogue /AcroForm key          a direct /Root carrying an /AcroForm,
#                                                              evade-acroform-inline-field
#   acroform-field        the whole form-field refusal         evade-widget-on-another-page, ...-no-widget
#   structure-tree        the whole structure-tree refusal     evade-struct-without-structparents,
#                                                              evade-structparents-resolving-elsewhere
#   inline-image-filtered the filtered-inline-image refusal    evade-inline-image
#   pattern-fill          the pattern-fill refusal             evade-image-as-pattern, ... (three)
#   type-three-paints     a Type 3 procedure's paint refusal   evade-paths-in-type3-glyph
#
# AND EACH NAMES THE TWINS IT MUST NOT FREE (#125 slice 5). A near-miss twin is `Ok` with the check
# in place and must stay `Ok` with it gone -- the pinning suite reports every fixture that moved,
# so a twin named in the output is a twin the deletion moved, and that fails the mutation too.
#
# EACH MUTATION IS ASSERTED TWICE BEFORE ITS RESULT COUNTS. That it applied: the anchor occurs
# exactly once and the file changed -- a replace that matched nothing is indistinguishable from a
# defence that holds. And that cargo REBUILT `burrow-engines` for it: cargo decides by mtime, and a
# red read off a binary built before the plant is the previous mutation's result, not this one's.
#
# The plants were MEASURED to compile warning-free under CI's `-D warnings` (this script does not
# set RUSTFLAGS; CI does): a deletion is a branch on `std::hint::black_box`, which the compiler
# cannot fold, so nothing becomes unreachable or unused.
# Red for a compile error is red for the wrong reason, and `require_red` would refuse it -- it
# matches on the fixture names, never on the exit status alone.
#
# AND THE TWIN CHECK ITSELF IS PROBED, on every full run. A spared list nothing can match passes
# every mutation, so after the mutations a COPY of this script, written beside it (a copy elsewhere
# resolves the repository wrongly and fails for that reason instead), lists an evasion the
# structure-tree deletion frees among that mutation's twins. The copy must fail NAMING it -- the
# FAIL line, not the exit status, which a broken copy would also give.
#
# Usage: tools/test-redaction-refusals-are-reached.sh [mutation ...]   (default: all of them, and
#        the twin-check probe; `twin-check-probe` runs the probe alone)

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

files=(
  core/burrow-engines/src/pdfsyntax/lexer.rs
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
  # IN CI A SKIP IS A FAILURE: the job that runs this has the engines, so their absence there is
  # the job broken, and a green that examined nothing is the outcome this script exists to refuse.
  [ -z "${CI:-}" ] || exit 1
  exit 0
fi

backup="$(mktemp -d)"
for i in "${!files[@]}"; do cp "${files[$i]}" "$backup/$i"; done
restore() {
  for i in "${!files[@]}"; do cp "$backup/$i" "${files[$i]}"; done
}
probe_copy=""
trap 'restore; rm -rf "$backup"; [ -z "$probe_copy" ] || rm -f "$probe_copy"' EXIT

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
    "acroform-field": (
        "core/burrow-engines/src/redact/steps.rs",
        "    pub(crate) fn refuse_form_fields(&self) -> Result<()> {\n",
        "    pub(crate) fn refuse_form_fields(&self) -> Result<()> {\n"
        "        if " + off + " {\n            return Ok(());\n        }\n",
    ),
    # THE FILTERED IMAGE IS READ AS SPLIT'S PRUNING READS IT -- the rest of the stream unread --
    # rather than refused, which is the refusal deleted and nothing else.
    "inline-image-filtered": (
        "core/burrow-engines/src/pdfsyntax/lexer.rs",
        "                InlineImages::Redaction { filtered, .. } => return Err(filtered()),\n",
        "                InlineImages::Redaction { filtered, .. } => {\n"
        "                    if " + off + " {\n"
        "                        self.extent_unknown = true;\n"
        "                        self.at = self.bytes.len();\n"
        "                        return Ok(());\n"
        "                    }\n"
        "                    return Err(filtered());\n"
        "                }\n",
    ),
    "pattern-fill": (
        "core/burrow-engines/src/pdfsyntax/geometry.rs",
        'b"scn" | b"SCN" if matches!(operation.operands.last(), Some(Operand::Name { .. })) => {',
        'b"scn" | b"SCN"\n                if matches!(operation.operands.last(), Some(Operand::Name { .. }))\n'
        "                    && !" + off + " =>\n            {",
    ),
    "type-three-paints": (
        "core/burrow-engines/src/pdfsyntax/geometry.rs",
        'b"f" | b"F" | b"f*" | b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" | b"sh" => {\n'
        "                return Refusal::TypeThreeProcedurePaints.refuse(",
        'b"f" | b"F" | b"f*" | b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" | b"sh"\n'
        "                if !" + off + " =>\n            {\n"
        "                return Refusal::TypeThreeProcedurePaints.refuse(",
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
# every one of the expected texts in its output and NONE of the twins in `spared` ('|'-separated,
# may be empty): a twin the pinning suite names is a twin the deletion moved.
require_red() {
  local name="$1" expects="$2" spared="$3"
  shift 3
  plant "$name"
  local out status
  set +e
  # NO COLOUR: CI sets CARGO_TERM_COLOR=always, which wraps `Compiling` in escape codes and made
  # the rebuild check below refuse every mutation as stale -- measured under ci-local. Colour is
  # not part of cargo's fingerprint, so this forces no rebuild of its own.
  out="$(CARGO_TERM_COLOR=never "$@" 2>&1)"
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
  local moved=() twin twins=()
  [ -z "$spared" ] || IFS='|' read -r -a twins <<<"$spared"
  for twin in "${twins[@]}"; do
    grep -qF "$twin" <<<"$out" && moved+=("$twin")
  done
  if [ "${#moved[@]}" -gt 0 ]; then
    echo "FAIL  $name: the deletion also moved its twin(s): ${moved[*]}"
    echo "$out" | tail -40
    fail=$((fail + 1))
    return
  fi
  echo "PASS  $name: rebuilt, red, naming ${#wanted[@]} expected and none of ${#twins[@]} twin(s): $expects"
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
        "nearmiss-property-list-inside-an-unused-type3-font.pdf:|nearmiss-nested-forms-no-oc.pdf:|nearmiss-oc-on-another-page.pdf:|nearmiss-annotation-without-a-layer.pdf:|nearmiss-image-without-a-layer.pdf:|nearmiss-property-list-inside-an-unused-pattern.pdf:|nearmiss-appearance-stream-without-a-layer.pdf:|nearmiss-appearance-state-without-a-layer.pdf:|nearmiss-span-over-an-untyped-dictionary.pdf:|nearmiss-span-over-an-untyped-group.pdf:" \
        $resolved ;;
    split-oc-form-route)
      require_red "$1" "a layer 2 forms down: not refused by the per-page check" "" \
        "${split[@]}" a_layer_at_every_depth_the_walk_follows_is_refused ;;
    split-oc-type3-route)
      require_red "$1" "a layer in a Type 3 glyph: not refused by the per-page check" "" \
        "${split[@]}" a_layer_inside_a_type_three_glyph_procedure_is_refused ;;
    image-in-region)
      require_red "$1" \
        "evade-image-in-form.pdf: redacted|evade-image-in-form-two-levels-down.pdf: redacted" \
        "nearmiss-image-outside-region.pdf:|nearmiss-image-in-form-outside-region.pdf:|nearmiss-image-in-form-two-levels-down-outside-region.pdf:" \
        $ink ;;
    vector-in-region)
      require_red "$1" \
        "evade-paths-in-form.pdf: redacted|evade-paths-in-form-two-levels-down.pdf: redacted" \
        "nearmiss-paths-outside-region.pdf:|nearmiss-paths-in-form-outside-region.pdf:|nearmiss-paths-in-form-two-levels-down-outside-region.pdf:" \
        $ink ;;
    shading-in-region)
      require_red "$1" "evade-shading-sh-in-form.pdf: redacted" \
        "nearmiss-shading-on-another-page.pdf:" $ink ;;
    form-route)
      require_red "$1" \
        "evade-image-in-form.pdf: redacted|evade-image-in-form-two-levels-down.pdf: redacted|evade-paths-in-form.pdf: redacted|evade-paths-in-form-two-levels-down.pdf: redacted|evade-shading-sh-in-form.pdf: redacted" \
        "nearmiss-image-in-form-outside-region.pdf:|nearmiss-image-in-form-two-levels-down-outside-region.pdf:|nearmiss-paths-in-form-outside-region.pdf:|nearmiss-paths-in-form-two-levels-down-outside-region.pdf:" \
        $ink ;;
    acroform-field-ft)
      # NOT "redacted": the suite's band reaches each widget's /Rect, so with the /FT scan gone
      # both fall to `[annotation-in-region]` -- moved off their rule, and named for it.
      require_red "$1" \
        "evade-widget-ft-on-parent.pdf: refused, but not by \`acroform-field\`|evade-field-without-acroform.pdf: refused, but not by \`acroform-field\`" \
        "nearmiss-annotation-with-a-parent-not-a-field.pdf:|nearmiss-widget-without-a-field-type.pdf:" \
        $resolved ;;
    acroform-field)
      # The two evasions that carry BOTH signals -- a catalogue /AcroForm and an /FT -- survive
      # either single deletion above, so the whole refusal is deleted here to show they reach it.
      require_red "$1" \
        "evade-widget-on-another-page.pdf:|evade-field-with-no-widget.pdf:" \
        "nearmiss-widget-without-a-field-on-another-page.pdf:|nearmiss-field-keys-without-a-field-type.pdf:|nearmiss-annotation-not-a-widget.pdf:|nearmiss-widget-without-a-field-type.pdf:|nearmiss-fields-the-catalogue-does-not-name.pdf:" \
        $resolved ;;
    acroform-catalogue)
      # Its ASSERTION message, not the test's name, which any failure of the test would print.
      # And the rule-pinning suite beside it (libtest ORs the two filters), so the inline field
      # this key alone catches is named, and its twin -- the same /Fields object, which no
      # catalogue /AcroForm names -- can be required not to move. The direct-/Root cases have no
      # twin that redacts: PDFium cannot open the output, so a direct /Root refuses on the second
      # reading whatever it carries (#125 slice 5, measured).
      require_red "$1" \
        "a direct /Root catalogue carrying an inline /AcroForm must refuse [acroform-field]|evade-acroform-inline-field.pdf:" \
        "nearmiss-fields-the-catalogue-does-not-name.pdf:" \
        "${defences[@]}" -- a_direct_root_catalogue_carrying_a_form_is_refused \
        a_named_property_list_and_a_layer_refuse_for_their_own_reason ;;
    structure-tree)
      # The rule-pinning suite, since #125 slice 5 put the structure-tree group in it: it names
      # each fixture that moved, so the twins without a tree can be required NOT to.
      require_red "$1" \
        "evade-struct-without-structparents.pdf:|nearmiss-structparents-but-nothing-in-region.pdf:|evade-structparents-resolving-elsewhere.pdf:" \
        "nearmiss-structparents-without-a-tree.pdf:|nearmiss-marked-content-without-a-tree.pdf:" \
        $resolved ;;
    inline-image-filtered)
      require_red "$1" "evade-inline-image.pdf:" \
        "nearmiss-inline-image-unfiltered-outside-region.pdf:" $ink ;;
    pattern-fill)
      require_red "$1" \
        "evade-image-as-pattern.pdf:|evade-image-in-tiling-pattern-inside-a-form.pdf:|evade-shading-pattern-fill.pdf:" \
        "nearmiss-image-pattern-never-selected.pdf:|nearmiss-tiling-pattern-never-filled.pdf:|nearmiss-shading-pattern-never-filled.pdf:" \
        $ink ;;
    type-three-paints)
      require_red "$1" "evade-paths-in-type3-glyph.pdf:" \
        "nearmiss-paths-in-type3-glyph-clip-only.pdf:" $ink ;;
    *)
      echo "unknown mutation: $1" >&2
      exit 2 ;;
  esac
}

# The probe: a copy beside this script with an evasion the structure-tree deletion frees written
# into that mutation's spared list. It must refuse, and say which twin "moved".
probe_twin_check() {
  local planted="evade-structparents-resolving-elsewhere.pdf:"
  probe_copy="$here/.test-redaction-refusals-are-reached.twin-probe.$$.sh"
  if ! python3 - "${BASH_SOURCE[0]}" "$probe_copy" "$planted" <<'PY'
import sys

source, copy, planted = sys.argv[1], sys.argv[2], sys.argv[3]
text = open(source, encoding="utf-8").read()
# Split so this line is not itself a second occurrence of the list it looks for.
old = '"nearmiss-structparents-without-a-tree.pdf:' + '|nearmiss-marked-content-without-a-tree.pdf:"'
assert text.count(old) == 1, f"the structure-tree spared list occurs {text.count(old)} times, not once"
probed = text.replace(old, '"' + planted + "|" + old[1:])
assert probed != text and probed.count(planted) == text.count(planted) + 1, "the probe did not apply"
open(copy, "w", encoding="utf-8").write(probed)
PY
  then
    echo "FAIL  twin-check-probe: the probe could not be planted in the copy"
    fail=$((fail + 1))
    return
  fi
  local out status want="FAIL  structure-tree: the deletion also moved its twin(s): $planted"
  set +e
  out="$(bash "$probe_copy" structure-tree 2>&1)"
  status=$?
  set -e
  rm -f "$probe_copy"
  probe_copy=""
  if [ "$status" -ne 0 ] && grep -qF "$want" <<<"$out"; then
    echo "PASS  twin-check-probe: a copy listing $planted as a twin refused, naming it"
    pass=$((pass + 1))
  else
    echo "FAIL  twin-check-probe: a copy listing $planted as a twin did not refuse naming it (exit $status)"
    echo "$out" | tail -20
    fail=$((fail + 1))
  fi
}

all=(optional-content split-oc-form-route split-oc-type3-route image-in-region vector-in-region
  shading-in-region form-route acroform-field-ft acroform-catalogue acroform-field structure-tree
  inline-image-filtered pattern-fill type-three-paints)
selected=("$@")
[ "${#selected[@]}" -gt 0 ] || selected=("${all[@]}" twin-check-probe)
for name in "${selected[@]}"; do
  if [ "$name" = twin-check-probe ]; then probe_twin_check; else run "$name"; fi
done

echo
echo "------------------------------------------------------------"
echo "$pass passed, $fail failed, of ${#selected[@]} check(s) (${#all[@]} mutations defined, and the twin-check probe)"
[ "$fail" -eq 0 ] && [ "$pass" -eq "${#selected[@]}" ]
