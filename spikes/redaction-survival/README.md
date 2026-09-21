# spike 0006 — what survives a redaction

Throwaway. Nothing here is linked, shipped or depended on by anything in `core/`, `bindings/` or
`apps/web/`. The report is [`docs/spikes/0006-what-survives-a-redaction.md`](../../docs/spikes/0006-what-survives-a-redaction.md);
this directory is how it was measured.

## Reproduce

```bash
# 1. the fixtures -- 22 channels plus one canary-free control, every one checked with qpdf --check
python3 spikes/redaction-survival/make-fixtures.py spikes/redaction-survival/results/fixtures

# 2. the measurement: four instruments, three states, four controls
cargo run --release --manifest-path spikes/redaction-survival/Cargo.toml

# 3. control 4 -- the redactor mutated to a no-op. Must exit 0.
cargo run --release --manifest-path spikes/redaction-survival/Cargo.toml -- --mutate-redactor-to-noop

# 4. finding 3 -- the SHIPPED split, run one-way over the same fixtures
cargo run --release --manifest-path spikes/redaction-survival/Cargo.toml --bin what-split-already-does

# 5. look at the pages, which is where the "a person sees it" column comes from
python3 spikes/redaction-survival/ppm2png.py --sheet out.png 3 results/pages/*-redacted.ppm

# and the surviving thumbnail, which no instrument in the allowed set can read
python3 spikes/redaction-survival/ppm2png.py results/pages/14-redacted-thumb.pgm
```

Needs the vendored engines (`engines/fetch.sh && engines/build-native.sh`). The build script does
**not** re-verify them against `engines/pins.toml`: that check exists so the production build
cannot link something nobody pinned, and a second copy of it here would be a second
implementation of a rule `core/CLAUDE.md` says must have exactly one.

## What is here

| | |
|---|---|
| `make-fixtures.py` | 22 hand-built, byte-exact PDFs, one per survival channel, plus the inertness control |
| `blockfont.py` | a generated 5×7 TrueType face, so the embedded-font channels rasterise and carry no licence |
| `src/main.rs` | the harness: instruments I1–I4, three states, four controls, the tables |
| `src/redact.rs` | the naive "remove the glyphs from the content stream" redaction — the strawman, written as the most plausible first attempt |
| `src/qpdf.rs`, `src/pdfium.rs`, `src/ffi.rs` | engine declarations made **here**, not in `core/`, because deciding which entry points the crate should grow is what the spike is for |
| `src/split_probe.rs` | finding 3: the real `burrow_ops::split`, one-way, over the same fixtures |
| `eyeball-verdicts.tsv` | the human "would a person see it" column. The harness **refuses to run** without a row per channel per state |
| `ppm2png.py` | PPM → PNG and contact sheets, so that column is an observation |
| `measurements/` | **committed.** The three run records, written by the binaries themselves — the evidence `docs/spikes/0006` cites |
| `results/` | everything else generated: fixtures, redacted PDFs, page images. Gitignored; regenerable from the two commands above |

Artifacts are split by mode: a plain run writes `results/redacted/`, and
`--mutate-redactor-to-noop` writes `results/redacted-mutated/`. They were **not** split until
review found it, and because the reproduce order above runs the mutation second, every "redacted"
PDF on disk was the output of a redactor that does nothing — a directory full of files that look
redacted and are not, in a spike about exactly that.

## Why the record is in `measurements/` and not `results/`

`tools/check-no-generated-files.sh` refuses any tracked path matching `(^|/)results/`, and the
probe it uses to prove that pattern is live is itself a spike results file from PR #37. A
`.gitignore` exception would not have helped — that gate scans the tracked tree, not
`.gitignore`, so the exception would have produced a committable path that fails CI.

The binaries write the record themselves rather than relying on a `>` in the commands above. A
redirect goes stale the first time somebody runs the binary without it, and then the file
disagrees with the run that produced it while looking exactly as authoritative — the same failure
as the no-op `redacted-*.pdf` directory above.

## Why `eyeball-verdicts.tsv` is a file and not a threshold

I4 measures ink in the region, which answers "is anything there", not "is the secret there".
Measured: channel 19 draws the secret and then a black rectangle over it, so I4 reports **ink
1.0000** on a page where nothing is readable — a higher number than channel 18, where the secret
is fully legible at **0.1361**. A threshold would have called the black bar "visible" and the
recommendation would have been built on it.

## The four controls

The run exits non-zero if any fails. Two of them refused a run during this spike, and a third
refused itself; the report records what each caught.

1. **non-vacuity** — every fixture carries its canary before any redaction.
2. **inertness** — the canary-free control comes back absent on every instrument.
3. **the keep line** — drawn outside the region in every fixture, must survive.
4. **the mutation** — with the redactor made a no-op, every channel must measure **identically to
   the round-trip**. Not "everything survives": one channel's secret is genuinely gone before the
   redactor runs, and the strict form needs no exception list.
