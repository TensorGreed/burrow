---
name: spike-0006-redaction-survival
description: Verified spike 0006 (what survives a redaction) on 2026-09-19 — the artifact traps, the two channels that fall through the recommendation, and the font-object leak class the 22 fixtures do not cover.
metadata:
  type: project
---

Spike 0006 `docs/spikes/0006-what-survives-a-redaction.md` + `spikes/redaction-survival/`
recommends redaction v1 rewrite page content streams only, with five refusals. Verified against
the harness output on 2026-09-19.

**Why:** the deliverable was the report's accuracy, not shipping code, and several of its claims
did not survive a recount.

**How to apply:** when M2 redaction is designed, these are the residuals to re-check rather than
re-derive.

- **`results/` is gitignored** (`.gitignore:81 spikes/**/results/`). Every evidence path the
  report cites — `results/run.txt`, `run-split-probe.txt`, `results/14-thumb.png` — is
  unreachable to a future reader. Only `eyeball-verdicts.tsv` is trackable. A spike whose bar
  says "every cell is a run, see results/run.txt" is pointing at nothing.
- **The mutation run overwrites the primary artifacts.** `src/main.rs` writes
  `results/redacted-NN-*.pdf` into `out_dir`, which does *not* vary with `--mutate-redactor-to-noop`
  (only `pages_dir` does). Running the reproduce block in the report's own order leaves every
  file named `redacted-*` holding the **no-op** output. Measured: `redacted-01-plain-tj.pdf`
  contains `(BURROW-SECRET-01) Tj` in the clear.
- **Channels 15 (`/Metadata` + `/Info`) and 16 (`/EmbeddedFiles`) fall through the
  handle/refuse/disclose assignment.** Both survive finding 1, both live on the catalogue,
  neither is in the refusal list. `split` removes them only because its build route cannot reach
  a destination catalogue — an inability redaction v1 does not inherit, because it edits in place.
- **The embedded-font object is a survival channel the 22 fixtures do not enumerate.** Measured:
  `/ToUnicode` in channel 5's output still spells `BURROW-SECRET-05` in order as bfchar entries;
  channel 4's `/Differences` still holds `/B /U /R /O /W /hyphen /S /E /C /T /zero /four`. The
  redactor rewrites content streams and never touches `/Resources`, so these survive — and the
  harness scores channels 4, 5, 6, 21 "still in the file: **no**".
- **`+3,340 brotli` is the wrong precedent to quote.** `engines/qpdf-not-exported.toml:119` says
  in the next sentence that merge's seven cost +53,456 and that "the variance is in what else
  gets pulled in, not in the count".

Related: [[m1_split_pruning]], [[m1_verify_output]], [[m1_engine_supply_chain]].

---

## Resolved, 2026-09-19, same day — every finding above was acted on

The findings are kept exactly as written, because a reader who only sees the repaired state
cannot tell which of them was real. All five were.

| finding above | what changed |
|---|---|
| `results/` is gitignored, so the evidence is unreachable | the binaries now write their own record to `spikes/redaction-survival/measurements/`, which **is** committed. Not a `.gitignore` exception: `check-no-generated-files.sh` refuses any tracked `(^|/)results/` path and scans the tracked tree, not `.gitignore`, so an exception would have produced a committable path that fails CI |
| the mutation run overwrites `redacted-*.pdf` | split into `results/redacted/` and `results/redacted-mutated/`; verified distinct |
| channels 15 and 16 fall through the assignment | **and the answer changed once it was measured properly.** A new channel 24 puts `/Metadata` and `/PieceInfo` on the *page*; `split`'s page-key allowlist removes both, and that mechanism is page-side and transfers. So v1 **strips** the page-level half and discloses only the catalogue |
| the embedded-font object is a survival channel | added as **channel 23**, with a structural probe (`src/fontscan.rs`) reading `/ToUnicode`, `/Differences` and the embedded sfnt's own `cmap`. It fires on channels 4, 5, 6, 9, 10 and 21. The headline moved from 12 of 22 to **17 of 23** |
| `+3,340 brotli` is the wrong precedent | now quoted as the **+3,340 to +53,456** range, with the source's own sentence about where the variance lives |

**The durable lesson is the control, not the list.** Four controls missed the font channel because
`survives()` was a disjunction of *witnesses* with no requirement that anything witness a
**removal** — so a channel where every instrument was blind and the human verdict said "no"
scored *gone* on no evidence at all. **Control 5**: if a channel is reported gone, something must
have changed between the before state and the after state; exempt only where the removal has a
witness named elsewhere. Every earlier control checks that something was *found*. That one checks
that something *moved*, and it is the only reason the number is 17.

Carry it to any leak harness, not only this one.
