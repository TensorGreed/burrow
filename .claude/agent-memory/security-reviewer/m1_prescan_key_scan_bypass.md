---
name: m1-prescan-key-scan-bypass
description: The structural pre-scan reads the FIRST literal match of a key in a 4 KiB window, so one decoy key defeats it entirely; measured M1 PR 3 (2026-09-10)
metadata:
  type: project
---

`core/burrow-engines/src/prescan/` decides whether a file is a declared-size bomb by byte
scanning for `/Size`, `/Index`, `/Prev` and `/Length`. Three properties of that scan are
each independently sufficient to bypass it, all measured on 2026-09-10 against
`tests/conformance/fixtures/xref-bomb.pdf` (refused: 4.7 ms, 5 MB peak):

- `xref::find_int` takes the **first** literal occurrence of the key and gives up if a
  digit does not follow. Inserting `/Sizes 1 ` before `/Size` in the xref stream
  dictionary — nine bytes — makes the scan report zero entries. PDFium then peaks at
  **2,506,408 KB and 7.1 s**.
- The dictionary window is 4096 bytes from the xref offset. Padding the dictionary past it
  hides `/Size` the same way, same measured cost.
- `startxref` is searched only in the last 2048 bytes. Appending 4000 bytes of junk after
  `%%EOF` makes the scan find no xref at all, same measured cost.

qpdf survives all three (269 MB, 0.68 s) because its global `flate_max_memory` of 256 MiB
fires — which is independent confirmation that `qpdf/limits.rs` really does apply.

**Why it matters beyond the numbers:** the pre-scan is the *only* pre-emptive memory
defence; `max_memory_bytes` on native is measured after the fact (see
[[m1-limits-real-strength]]). A byte scan that can be steered by one decoy token is a
signature check, not a bound. The durable fix is to look for the key in the last position
that a PDF dictionary parser would resolve, or to treat "a `/Type /XRef` dictionary whose
`/Size` could not be read" as suspicious rather than as zero.

**How to apply:** whenever a check is implemented as "find token, read number nearby",
ask what the attacker can put *before* the token, and test the decoy immediately — it is a
two-minute experiment with a large payoff. See [[user-role]].
