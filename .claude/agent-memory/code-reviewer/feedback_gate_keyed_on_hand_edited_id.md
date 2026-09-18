---
name: feedback-gate-keyed-on-hand-edited-id
description: Review technique — when a burrow gate only fires for items it can resolve from a hand-maintained id (artifact path, component name), mutate that id and check the gate turns itself off while the tool still prints OK
metadata:
  type: feedback
---

A check that looks up its expectations by an identifier read from a hand-maintained file
(`engines/licenses.toml`'s `[[artifact]] files`, a component `name` that must substring-match a
detector fingerprint) **silently stops gating anything it cannot resolve**. Mutate the id, not
the data, and read the verdict line.

**Why:** measured on `tools/make-sbom.py` (2026-09-18). Its native `linked_in` equality gate is
the only hard gate over the shipped binaries. Rewriting one path string in `[[artifact]] files`
so nothing names `libpdfium.so` made the tool emit `note: ... scanned but no [[artifact]] names
it` and then print `OK`. Separately, 8 of 23 declared components have `linked_in` claims and no
detector fingerprint, so the "declared and not in the binary" half of the gate is inert for them
— a false `linked_in` on `fast_float` passes CI once the SBOM is regenerated.

**How to apply:** for each gate, ask *what must resolve for this to fire*, then break that
resolution and re-run. Expect the unresolvable case to be a **problem**, not a note. This is the
same failure family as [[feedback-inert-detector-path]]: the check examines nothing and says so
in a line the reader skips. Related: [[feedback-review-expectations]].
