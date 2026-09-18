---
name: feedback-inert-detector-path
description: Review technique — when a burrow check wraps detect-engine-components.py (or any detector), monkeypatch its detect() to return nothing and see whether the wrapper still prints OK
metadata:
  type: feedback
---

When a new checker consumes another tool's detection results, do not only test "tool absent"
and "tool finds something". **Force the detector to find nothing while its inputs are fully
present**, and read what the wrapper prints.

**Why:** measured on `tools/make-sbom.py` (2026-09-18). Its third source is
`detect-engine-components.py`. With `engines/vendor/` fully populated but every fingerprint
inert, it printed *"detector: NO artifacts scanned -- the vendor tree is absent"* and exited 0
with *"OK -- the SBOM, the engine manifest and the shipped binaries agree."* — a false statement
plus a pass, which is this repository's most-repeated failure mode. The cause was counting
detected **labels** and calling the count "artifacts scanned", and not calling the detector's
own `check_probes()`.

**How to apply:** load the tool with `importlib` from the scratchpad and patch the loaded
detector module's `detect` to `lambda syms, strs: {}` (patch the wrapper's `load_detector`, it
re-imports per call). Then check: does the message distinguish "no artifacts" from "no hits",
and does the wrapper run the detector's probe gate? Related:
[[feedback-fuzz-seed-prefix-check]], [[feedback-review-expectations]].
