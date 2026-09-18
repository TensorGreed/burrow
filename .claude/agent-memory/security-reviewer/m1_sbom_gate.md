---
name: m1-sbom-gate
description: What tools/make-sbom.py --check actually binds - measured across two passes: the first pass's holes are closed, but the new native linked_in equality gate is defeated by a libjpeg/libopenjpeg label collapse and does not run at all on an x86_64 runner.
metadata:
  type: project
---

`tools/make-sbom.py` (branch `sbom-and-strip-gate`) generates `sbom/burrow.cdx.json` from
`cargo metadata --locked --all-features`, `engines/licenses.toml`, and
`tools/detect-engine-components.py` imported as a module.

**Why:** it is the licence/supply-chain gate. Its failure mode is passing while examining
nothing, which this repository has measured repeatedly.

**How to apply:** re-measure these when the file, the detector or `engines/licenses.toml`
changes. Do not re-derive them from the module docstring — the docstring has overclaimed twice.

## Closed by the second pass (measured 2026-09-18, aarch64 dev box, full vendor tree)

- `check()` now calls `detector.check_probes()` first; the inert-fingerprint hole is closed and
  `tools/test-make-sbom.sh` case 6 catches it behaviourally (mutation asserted before use).
- An absent vendor tree fails; `--allow-no-artifacts` is the only opt-out.
- Rule 2 compares the manifest against the *committed* document, so it can fire.
- Regeneration is byte-identical to the committed file; no timestamp, serialNumber, hostname,
  username or absolute path in the output. Package set is lockfile-derived, machine-independent.
- Self-test: 11/11 pass on aarch64, `EXPECTED_CASES=11` matches the ok/bad call count.

## Open, measured

- **Label collapse defeats the new equality gate and the detector both, for `libjpeg`.**
  `libjpeg`'s `manifest_names` is `("libjpeg", "jpeg")`, and `"libopenjpeg"` contains `jpeg`, so
  the `libopenjpeg` entry supplies the `libjpeg` label to `linked_labels()` and to
  `declared_labels()`. Measured: delete the whole `libjpeg-turbo (bundled in pdfium)` component
  (IJG, mandatory attribution, genuinely in `libpdfium.so`), regenerate, and
  `make-sbom.py --check` exits **0** and `detect-engine-components.py` exits **0**. Same result
  for the weaker mutation of emptying only its `linked_in`. Self-test case 8a uses `lcms`, which
  has no collision, so the suite cannot see this.
- **The native `linked_in` gate does not run on an x86_64 runner.** `artifact_ids()` matches the
  scanned path against `[[artifact]].files`; the manifest names only
  `vendor/native-aarch64/lib/libpdfium.so`, while `engines/build-native.sh` writes
  `vendor/native-$(uname -m)/`. On `ubuntu-latest` the path is `native-x86_64`, `ids` is `[]`,
  the gate is skipped and a *note* is printed. Simulated by rewriting the manifest's `files`
  string: `--check` still prints OK with case 8a's defect present.
  Consequence for the branch: `test-make-sbom.sh` cases 8a/8b hard-code `native-aarch64` in
  their needles, so they should go red on `ubuntu-latest`.
- **The write-path guard in `main()` is unreachable.** `SBOM.parent != REPO/"sbom" and
  "BURROW_SBOM" not in os.environ` — the only way the parent can differ is `BURROW_SBOM`, which
  also falsifies the second clause. Enumerated over unset / outside-repo / empty / in-repo: never
  fires.
- `--check` prints counts but never prints **which** manifest, SBOM or vendor tree it read; the
  no-artifacts message hard-codes "engines/vendor" even under `BURROW_ENGINE_VENDOR`.
- `tools/test-make-sbom.sh` writes `tools/make-sbom.MUTANT.py` beside the original (correct, for
  `REPO` resolution) and removes it on an EXIT trap. It is not in `.gitignore` and matches no
  pattern in `check-no-generated-files.sh`.
- `BURROW_ENGINE_VENDOR` / `BURROW_ENGINE_LICENSES` / `BURROW_SBOM` are unvalidated env
  overrides. No workflow sets them and there is no `pull_request_target` or `inputs.`
  interpolation into env, so they are not attacker-reachable in CI; nothing asserts they are
  unset when running under CI either.
- `check-engine-licences.py` does **not** honour `BURROW_ENGINE_LICENSES` (it re-reads the real
  manifest), unlike the other two. Do not use it to cross-check a mutated manifest.
- Still true from the first pass: `find_artifacts()` only globs `native-*/lib/libpdfium.so` and
  `wasm/lib/*.wasm`; the vendored `libqpdf.a`, `libz.a`, `libjpeg.a` are never scanned.
  Components with no fingerprint (pdfium, qpdf, abseil, sphlib, rijndael, fast_float, simdutf,
  libc++, Emscripten) are outside every symbol-derived check.

Related: [[m1-engine-supply-chain]], [[ci-local-preflight]].
