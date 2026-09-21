# Spike 0006 — what survives a redaction, and what a scan can find afterwards

**Date:** 2026-09-19
**Status:** complete — recommends that redaction v1 rewrite **page content streams and the font
mappings that describe them**, **strip every page-level key outside a named allowlist**, refuse
five document shapes outright, disclose three it cannot reach, and that its verification assert
**what it did**, never *that the secret is gone*. No ADR is amended here.

**[ADR 0029](../adr/0029-what-redaction-does-and-what-it-refuses-to-do.md) is written from this
spike and was accepted on 2026-09-21.** Said on this document's own status line because that is where
this repository puts it (ADRs 0002, 0007, 0008 and spike 0004), not only in the record that
decides it. Three things in that ADR are not in this report and are not derivable from it: the
per-channel assignment is stated as a decision rather than a recommendation; control 5 is
generalised into a project rule; and **#111's shape is identified a second time, in geometry** —
this spike measured the `/ToUnicode` half and never asked whether the code that decides *where a
glyph is* is checked by anything other than itself. It is not, and ADR 0029 §6 names the
test-time oracle for it.

Spike code: `spikes/redaction-survival/`, excluded from the cargo workspace by its own
`[workspace]` table. Nothing is in `core/`, `bindings/` or `apps/web/`.

Reproduce:

```bash
python3 spikes/redaction-survival/make-fixtures.py spikes/redaction-survival/results/fixtures
cargo run --release --manifest-path spikes/redaction-survival/Cargo.toml
cargo run --release --manifest-path spikes/redaction-survival/Cargo.toml -- --mutate-redactor-to-noop
cargo run --release --manifest-path spikes/redaction-survival/Cargo.toml --bin what-split-already-does
```

**Every number below is from `spikes/redaction-survival/measurements/`**, which the harness
writes itself on every run and which is **committed**. See *The record* at the end for why it is
not in `results/` and why a shell redirect was not good enough. Timings move a few percent
between runs; everything else is deterministic.

### Read this first: the first version of this report was wrong about its own headline

**It said 12 of 22. It is 17 of 23**, and the four it missed are the ones this project has warned
about in three separate files since M1. A security review of the report caught it; the correction
is *Finding 1a*. The cause was not an arithmetic slip: every instrument the spike had was looking
for the secret **as text**, and the four missed channels retain it as a **font mapping**. The
enumeration asked *where is the text drawn from* and never asked *what does the drawing apparatus
keep*.

That review also found that the spike's artifact directory held files that looked redacted and
were not — in a spike about exactly that. Both are recorded where they happened rather than
smoothed out.

---

## The bar

Set before anything was measured.

- [ ] Every enumerated survival channel has a **fixture**, and a non-vacuity control proves each
      one carries its canary before any redaction is applied.
- [ ] Every (channel × instrument) cell is a **run**, not an argument.
- [ ] A page content stream is read, rewritten and written back using **only** functions on
      `engines/qpdf-trapped-functions.txt`.
- [ ] Every instrument's cost is measured, medians of 11, so the render route is priced rather
      than skipped.
- [ ] Every channel is classified against the machinery that already exists.
- [ ] Every channel carries **both** verdicts — is it still in the file, and would a person see
      it — with neither inferred from the other.
- [ ] The recommendation assigns every channel to **handle / refuse / disclose**, none unassigned.

## Method

23 hand-built fixtures, one per place the visible text of a page can also exist, plus a
canary-free control. All share one shape: a 400×200 page, the secret drawn at (40, 100) inside
the redaction region `[30 88 370 132]`, and a `KEEP-THIS-LINE` outside it that must survive — a
redactor that deletes the whole page passes every "is the secret gone" test ever written.

Four instruments, all permitted under this project's licence rule. **poppler was considered and
excluded**: it is GPL, and a GPL tool's output in `docs/` becomes the precedent for the next one.
No channel turned up that only poppler would have seen.

| | instrument | what it is |
|---|---|---|
| **I1** | raw byte scan of the emitted file, ASCII + UTF-16BE, with and without BOM, hex and literal | the naive check ADR 0022 describes |
| **I2** | `qpdf --qdf --object-streams=disable`, then I1 | exactly what `core/burrow-ops/tests/split_no_leak.rs` does today |
| **I3** | PDFium `FPDFText_LoadPage` / `CountChars` / `GetUnicode` / `GetText` | decodes **through the font** |
| **I4** | PDFium raster, ink measured over the region, at `flags = 0` **and** at `FPDF_ANNOT` | what a person sees |

Plus **two structural probes**, which are not instruments and are labelled as such throughout:
they read named constructs rather than searching, so they find what they were pointed at and
nothing else.

| | probe | reads |
|---|---|---|
| **P1** | the page's `/Thumb` | presence, dimensions, ink, and the pixels, written out as a file |
| **P2** | the fonts (channel 23) | `/ToUnicode` `beginbfchar` targets, `/Differences` glyph names, and the embedded sfnt's own `cmap` |

**P2 did not exist in the first version of this spike, and its absence is this report's largest
error.** See finding 1a.

Three states are compared rather than two. The fixtures are uncompressed by construction, so
scanning one measures the generator; everything is measured against a **qpdf round-trip** of the
fixture — open and write, no edit — so the measurement and the control differ by the redaction
and nothing else. It also turned out to matter: on one channel the round-trip removes the secret
by itself, which a two-state harness would have credited to the redaction.

### The controls, and the three that fired

Controls 1, 2, 3 and 5 run on every invocation; control 4 runs only under its flag. The harness
exits rather than warning.

| | control | what it caught |
|---|---|---|
| 1 | **non-vacuity** — every fixture's canary must be findable before any redaction | **fired.** The `/Differences` fixture was built with `/Flags 4` (symbolic), which makes a reader ignore `/Differences` entirely and consult the font's own cmap — the page drew *nothing*, and "the secret did not survive" would have been recorded for a fixture that never had one |
| 2 | **inertness** — a canary-free control must come back absent on every instrument | 22 of 22, every run |
| 3 | **the keep line** must survive in every channel | 22 of 22 |
| 4 | **the mutation** — `--mutate-redactor-to-noop` | **fired on itself.** The first version asserted "every channel survives" and failed channels the instruments cannot see in any state. The sharp form is that a no-op redaction must leave the measurement **identical to the round-trip's** for every channel; that passes 22 of 22 and needs no exception list |
| 5 | **a removal must have been observed** — a channel scored *gone* must differ from the round-trip on at least one instrument | **added after review, and it is the control that would have caught finding 1a** |

**Control 5 is the one worth carrying out of this spike.** The harness scores a channel
"survives" as a disjunction of witnesses — any scan, or the human verdict, or a probe. A channel
where every instrument is blind *and* the human wrote "no" therefore scores **not in the file**
with no evidence whatever that it went. Control 1 cannot catch that: it keys on the *fixture*,
where the raw scan fires on uncompressed generator output. Control 4 cannot catch it: a no-op
leaves both states equally blind, so they agree.

So: **if a channel is reported gone, something must have changed between the round-trip and the
redacted state.** A channel already gone at the round-trip is exempt, because its witness is
named elsewhere — finding 1's last column. Everything else must have been seen to go.

That rule, written down, is what channel 23 was hiding behind. It is the same shape as
`CLAUDE.md`'s *"a check that silently examines nothing is worse than no check"*, one level in:
**a removal nothing observed is not a measured removal.**

One correction to an earlier draft's history: control 1 originally keyed on the **round-trip**
state, where channel 17 failed it. The remedy was to re-key it onto the **fixture** — which is a
*weakening* (a canary the round-trip destroys now passes non-vacuity), and the earlier draft
presented that firing as a finding without saying so. It is compensated by finding 1's "removed
by the round-trip alone" column, which is where channel 17's witness now lives.

The naive redaction is **run, not reasoned about**: it tokenises the page's content, finds every
`BT`…`ET`, and deletes the whole text object when its origin falls inside the region. That is
the most plausible first attempt, written as such.

### Two defects in the harness that review found, recorded where they happened

1. **Every file in `results/` named `redacted-*.pdf` was the no-op output.** The page-image
   directory varied with `--mutate-redactor-to-noop`; the PDF output directory did not, and the
   reproduce sequence above runs the mutation *second*. So the artifact directory of a spike
   about files that look redacted and are not was a directory of files that looked redacted and
   were not. Now `results/redacted/` and `results/redacted-mutated/`.
2. **`results/14-thumb.png` was cited by the report and produced by nothing.** It had been made
   by an ad-hoc command that was never saved, and the clean reproduction deleted it. P1 now
   writes the decoded pixels itself. Fixing that exposed a second defect: `qpdf_oh_get_key` on a
   **stream** handle returns null — `getKey` wants a dictionary — so the probe read `/Width` as 0
   and silently wrote nothing. `qpdf_oh_get_dict` is the seam.

### What these fixtures can and cannot support

Per `CLAUDE.md`: *a test harness that generates its own inputs is measuring what it can
generate.*

- **The font is generated** (`spikes/redaction-survival/blockfont.py`), a 5×7 block face with
  real outlines, so the fixtures rasterise and the "would a person see it" column is an
  observation. It has no ligatures, no kerning pairs, no contextual substitution and no vertical
  writing. **Any channel that depends on those is not covered here.**
- **The font fixtures are worst case for channel 23.** The secret is the only text in its font,
  so the subset's alphabet *is* the secret's alphabet. Where the same font also sets other text,
  the disclosure is the union and is correspondingly weaker. The shared case is **not measured**,
  and the recommendation says so.
- **P2 reads three constructs of at least four.** `/ToUnicode`, `/Differences` and the sfnt
  `cmap`. It does **not** read the `post` table's glyph-name indices or the `glyf`/`loca`
  inventory, both of which also describe the subset. Channel 23's row is a floor.
- **One page, one secret, one region.** Text straddling a region boundary, text under a clip
  path, and multi-page interaction are not covered. Those are properties of a *redactor*.
- **No encryption and no signatures.**
- Channel 21's lying `/ToUnicode` maps **per glyph**, so a repeated letter gets one mapping and
  the decoy comes back slightly mangled (`HAIILEES-FI-LER` rather than `HARMLESS-FILLER-`). The
  point — extraction reads something that is not the secret — holds; the clean decoy does not.

---

## Finding 1 — a naive content-stream redaction leaves the secret in the file in **17 of 23** places, and plainly on the page in **3**

`text objects cut` is the redactor's own count. `still in the file` means the secret is present
by any means — including means no instrument can read. `a person sees it` is a human verdict in
`spikes/redaction-survival/eyeball-verdicts.tsv`, not a threshold: see finding 2 for why it could
not be a threshold.

| # | channel | cut | still in the file? | a person sees it? | removed by the qpdf round-trip alone? |
|--:|---|--:|:--:|:--:|:--:|
| 1 | plain `Tj`, unembedded font | 1/2 | no | no | no |
| 2 | one `TJ` array, kerned between every glyph | 1/2 | no | no | no |
| 3 | one `Tj` per glyph, repositioned by `Tm` | 1/2 | no | no | no |
| 4 | subset font, codes via `/Differences`, no `/ToUnicode` | 1/2 | **yes** | no | no |
| 5 | Identity-H CID font, honest `/ToUnicode` | 1/2 | **yes** | no | no |
| 6 | the same, **no `/ToUnicode`** | 1/2 | **yes** | no | no |
| 7 | text inside a **Form XObject** | 0/1 | **yes** | **yes** | no |
| 8 | text inside a **Type 3 glyph procedure** | 1/2 | **yes** | no | no |
| 9 | **`/ActualText`** on the marked-content span | 1/2 | **yes** | no | no |
| 10 | **`/StructElem` `/Alt` + `/ActualText`** | 1/2 | **yes** | no | no |
| 11 | **annotation** `/Contents` + `/AP /N` | 0/1 | **yes** | **viewer only** | no |
| 12 | **AcroForm field `/V`** + widget appearance | 0/1 | **yes** | **viewer only** \* | no |
| 13 | inside an **optional-content group that is OFF** | 1/2 | **yes** | no | no |
| 14 | the page's **`/Thumb`**, as pixels | 1/2 | **yes** | **viewer only** | no |
| 15 | catalogue **`/Metadata`** XMP + `/Info` | 1/2 | **yes** | no | no |
| 16 | **`/EmbeddedFiles`** attachment | 1/2 | **yes** | no | no |
| 17 | **incremental update**: the superseded stream | 0/1 | no | no | **yes** |
| 18 | drawn as **filled rectangles** — no font, no text object | 0/1 | **yes** | **yes** | no |
| 19 | drawn, then a **black rectangle** over it | 1/2 | no | no | no |
| 20 | inside a **`/DCTDecode` image** | 0/1 | **yes** | **yes** | no |
| 21 | Identity-H whose **`/ToUnicode` lies** | 1/2 | **yes** | no | no |
| 22 | one text object **straddling two `/Contents` streams** | 1/2 | no | no | no |
| **23** | **the font's own mapping of the removed run** | — | **yes**, on 4, 5, 6, 9, 10, 21 | no | no |
| **24** | **`/Metadata` and `/PieceInfo` on the PAGE**, not the catalogue | 1/2 | **yes** | no | no |

\* Channel 12's "viewer only" is **asserted, not measured**: the `/AP /N` is there and viewers
draw it, but PDFium does not draw a **widget** even with `FPDF_ANNOT` — it needs a form-fill
environment this harness does not build. Marked in `eyeball-verdicts.tsv` and carried here and
into the refusal table, because a reader who reads only finding 1 and the recommendation would
otherwise never learn the cell is an assertion.

**Six channels the naive redactor never even reaches**, because there is no text object at the
region's origin in the page's own content: 7, 11, 12, 17, 18, 20. Their `cut` column reads `0/1`
— one text object on the page, the keep line, correctly left alone.

**The two columns come apart in both directions, and that is the finding the recommendation
turns on.**

- Channels 18 and 20 are **plainly legible and invisible to every scan**. The secret is drawn as
  path fills and as JPEG pixels; there is no text anywhere.
- Channel 14 is **legible in a viewer's thumbnail pane and invisible to all four instruments**.
  Only P1 found it. The surviving `/Thumb`, written out of the redacted output by the harness and
  enlarged, reads `BURROW-CARRIER-14`: `results/pages/14-redacted-thumb.pgm`.
- Channels 9, 10, 15 and 16 are the reverse: **present and invisible to a person**.

**Channel 17's history is closed, and the true claim is narrower than "incremental updates are
safe".** The superseded content stream is gone because **qpdf's writer emits only objects
reachable from the current trailer**, and this fixture's update replaces *the same object
number*. That is one shape. It does **not** measure an update that adds a new object while the
old one stays referenced, an old page object reachable through a `/Prev` chain, or a file whose
cross-reference is damaged and which qpdf reconstructs by scanning the body — where the old
object can be picked up. An earlier draft said redaction does not have to close incremental-update
history, in the same breath as calling it *"the channel most redaction advice warns about"*. That
is the worst place in this document to generalise from one fixture, and it is withdrawn.

**Channel 19 is the anti-pattern, and it behaves exactly as the roadmap says it does.** Before
the redaction the page shows a solid black bar and `qpdf --qdf` and PDFium both read
`BURROW-SECRET-19` straight out from under it. *"A black rectangle over text is not redaction"*
is now a measurement in this repository rather than a slogan.

## Finding 1a — channel 23: the font keeps a description of the text after the text is gone

**This is the channel the first version of this report did not have, and it is the one the
repository has been warning about since M1.** Three separate files carry the sentence *"a font
subset still carrying glyphs for removed characters … for redaction that transformed form **is**
the leak"* — `verify.rs`, `prune/mod.rs`, `object_closure.rs`. The spike built four fixtures that
exhibit it and measured past all four, because every instrument it had was looking for text.

A subset font built for one run carries, in the document, a description of that run:

| construct | what it discloses | measured in the redacted output |
|---|---|---|
| `/ToUnicode` `beginbfchar` | one entry per **position**, so it spells the run **in order** | channel 5: `0042 0055 0052 0052 004F 0057 002D 0053 0045 0043 0052 0045 0054 002D 0030 0035` — **`BURROW-SECRET-05`, complete** |
| `/Differences` | each code's glyph **name**, in first-appearance order | channel 4: `[1 /B /U /R /O /W /hyphen /S /E /C /T /zero /four]` — the alphabet, 12 of 12 |
| the embedded sfnt's `cmap` | the character codes the subset covers — on a single-run subset, **the alphabet** | channel 6: `-06BCEORSTUW`; channel 21: `-12BCEORSTUW` |

The page in each case has been correctly redacted:
`results/redacted/redacted-05-cid-with-tounicode.pdf`'s content stream is
`BT /Helv 14 Tf 40 40 Td (KEEP-THIS-LINE) Tj ET` and nothing else. The glyphs are gone and the
document still says what they said.

**Channel 21 is the sharpest result in this spike.** Its `/ToUnicode` lies, so text extraction
reads a decoy — and the font's own `cmap` covers exactly `BURROW-SECRET-21`'s alphabet anyway.
**The lie protects the secret from the extraction instrument and does not protect it from the
font.** Whatever a producer claims its glyphs mean, the subset it shipped still says which
characters it was built for. Two halves of the same font object disagree, and neither is
authoritative.

**Removing the text-showing operator removes none of this.** It is not a scanner gap to be closed
by a better scanner; it is a channel, and closing it means editing the font objects.

## Finding 2 — after the redaction, **3 of the 17 survivors are invisible to every scan**, and one to all four instruments; before it, **6 of 23 are invisible to every scan**

### After the redaction

| # | channel | I1 raw | I2 `--qdf` | I3 PDFium text | carrier canary | **P2, the font** | I4 ink, flags=0 | I4 ink, `FPDF_ANNOT` | P1 `/Thumb` ink |
|--:|---|:--:|:--:|:--:|:--:|:--:|--:|--:|--:|
| 4 | differences-encoding | no | no | no | — | **alphabet 12/12** | 0.0000 | 0.0000 | — |
| 5 | cid-with-tounicode | no | no | no | — | **verbatim** | 0.0000 | 0.0000 | — |
| 6 | cid-without-tounicode | no | no | no | — | **cmap alphabet** | 0.0000 | 0.0000 | — |
| 7 | form-xobject | no | **yes** | **yes** | — | — | 0.0717 | 0.0717 | — |
| 8 | type3-glyph | no | **yes** | no | — | — | 0.0000 | 0.0000 | — |
| 9 | actualtext | no | no | no | **yes** | **alphabet 12/12** | 0.0000 | 0.0000 | — |
| 10 | structure-tree | no | no | no | **yes** | **alphabet 12/12** | 0.0000 | 0.0000 | — |
| 11 | annotation | no | **yes** | no | **yes** | — | 0.0000 | **0.0693** | — |
| 12 | acroform-field | no | **yes** | no | **yes** | — | 0.0000 | 0.0000 | — |
| 13 | optional-content | no | no | no | **yes** | — | 0.0000 | 0.0000 | — |
| 14 | thumbnail | no | no | no | no | — | 0.0000 | 0.0000 | **0.360** |
| 15 | metadata | no | no | no | **yes** | — | 0.0000 | 0.0000 | — |
| 16 | attachment | no | no | no | **yes** | — | 0.0000 | 0.0000 | — |
| 18 | vector-outlines | no | no | no | — | — | **0.1361** | 0.1361 | — |
| 20 | image-pixels | no | no | no | — | — | **0.3591** | 0.3591 | — |
| 21 | cid-lying-tounicode | no | no | no | — | **cmap alphabet** | 0.0000 | 0.0000 | — |
| 24 | page-level-metadata | no | no | no | **yes** | — | 0.0000 | 0.0000 | — |

The six channels where nothing survives are omitted. **Their cells are not uniformly zero, and
an earlier draft's parenthetical saying so was false about the row it most needed to be true
about**: channel 19 reads `no | no | no | — | — | 1.0000 | 1.0000 | —`. A black bar is ink.

Four things this table says, in order of how much they cost.

**I1 never fires. Not once, in any state.** qpdf's writer compresses streams, so a raw byte scan
of an emitted document finds nothing whatever is in it. A verification built on scanning output
bytes decompresses first, or it is nothing.

**I3 finds one of the seventeen.** PDFium's text page reads the *page's* text, and sixteen of the
seventeen survivors are not on the page's text layer. The one it finds — channel 7's Form XObject —
it finds because a form's content **is** page text as far as extraction is concerned.

**The carrier canary is what makes this table attributable**, and it was added after the first
run could not be read. Channels 9–16 originally drew the secret on the page *and* put it in the
carrier, so when something removed the carrier the scan still found the page copy.

**`FPDF_ANNOT` moves exactly one row**, and it is the row that matters most for what burrow can
show a person. Channel 11's FreeText appearance draws at ink 0.0693 with the flag and 0.0000
without — and burrow's own render passes **zero** (`core/burrow-engines/src/pdfium/ffi.rs`,
*"annotations are not part of the page"*). A redaction preview built on the shipped render would
show a clean page over an annotation still carrying the secret.

### Before the redaction — the half that decides what verification can mean

**The I2 and I3 columns here are the PAGE canary**, except where the row says otherwise.
Channels 9–16 also carry a second canary in the non-page carrier, and channel 9 is the one where
the two disagree: its page glyphs are invisible to every scan while its `/ActualText` is not, so
it is *not* counted among the six below. The `no (page)` / `carrier only` cells for 9 and 10 are
this report reading two of the harness's columns together, not a column the harness prints.

| # | channel | I2 | I3 | a person sees it |
|--:|---|:--:|:--:|:--:|
| 1 | plain-tj | yes | yes | yes |
| 2 | kerned-tj | **no** | yes | yes |
| 3 | positioned-runs | **no** | **no** | **yes** |
| 4 | differences-encoding | **no** | yes | yes |
| 5 | cid-with-tounicode | **no** | yes | yes |
| 6 | cid-without-tounicode | **no** | **no** | **yes** |
| 7 | form-xobject | yes | yes | yes |
| 8 | type3-glyph | yes | **no** | yes |
| 9 | actualtext | no (page) | carrier only | yes |
| 10 | structure-tree | no (page) | yes | yes |
| 11 | annotation | yes | **no** | viewer only |
| 12 | acroform-field | yes | **no** | viewer only \* |
| 13 | optional-content | yes | yes | no |
| 14 | thumbnail | yes | yes | yes |
| 15 | metadata | yes | yes | yes |
| 16 | attachment | yes | yes | yes |
| 17 | incremental-update | **no** | **no** | no |
| 18 | vector-outlines | **no** | **no** | **yes** |
| 19 | covered-by-a-rectangle | yes | yes | no |
| 20 | image-pixels | **no** | **no** | **yes** |
| 21 | cid-lying-tounicode | **no** | **no** | **yes** |
| 22 | split-content-streams | yes | yes | yes |

**Five channels — 3, 6, 18, 20, 21 — are plainly legible to a person and invisible to every
scan, before anything is redacted.** (I4 sees ink on all five, as it sees ink on a black bar;
channel 19 is why that is not the same as seeing the secret.) That is the number the whole
recommendation rests on. On those five, a scanning verifier cannot establish that the secret was
*ever there*, so it cannot report that it went.

What PDFium's text page actually reads is worth having verbatim, because three rows explain
themselves:

| # | channel | before |
|--:|---|---|
| 3 | positioned-runs | `BURROW- SECRET- 03 KEEP-THIS-LINE` — **PDFium inserts separators between positioned runs**, so a contiguous match fails on text it read perfectly well |
| 8 | type3-glyph | `a KEEP-THIS-LINE` — extraction reads the Type 3 font's *character code*, never the text its glyph procedure draws |
| 21 | cid-lying-tounicode | `HAIILEES-FI-LER KEEP-THIS-LINE` — the CMap's claim, not the glyphs' meaning |

Row 3 is the cheapest lesson here: **a substring match is not text extraction**. The text was
extracted; the *matcher* failed. A redaction verifier that looked for its secret this way would
report success on a page that plainly still says it.

Row 21 is the expensive one, and finding 1a is why. `/ToUnicode` is *the* mechanism by which a
PDF says what its glyphs mean, and it is written by the producer of the file — which, for a
document somebody is asking burrow to redact, is not burrow. Channel 9 shows the same thing
through a second door: PDFium honours `/ActualText` and returns it *instead of* the glyphs.

### Instrument cost — medians of 11, native aarch64, from `measurements/run.txt`

| document | pages | I1 raw | I2 `--qdf` | I3 PDFium text | I4 render 4× |
|---|--:|--:|--:|--:|--:|
| one 400×200 page, round-tripped | 1 | **9 µs** | 1.67 ms | **117 µs** | 4.69 ms |
| `tests/conformance/fixtures/pages-137.pdf` | 137 | 123 µs | 3.34 ms | **330 µs** | **3,786 ms** |

These move a few percent between runs. An earlier draft quoted a third run that was not kept,
which is exactly the standard this spike holds everything else to; the numbers above are the ones
in the committed output, and the conclusions do not turn on the third significant figure.

**The render route is priced, and the price is the finding.** Roughly 3.8 seconds for 137 *empty*
pages at 4×, call it 27–28 ms per blank US-Letter page, and ADR 0027 §2a already measured a
*single* 240×320 thumbnail of a path-heavy page at 34 seconds and 819 MB. Rendering every page of
a document to check a redaction is not a verification step; it is a second operation, larger than
the first.

**What is unbounded here is not the raster.** `max_pixels` is exact and does bound the bitmap
(`CLAUDE.md`, ADR 0027 §1). What nothing in `Limits` bounds is the **work per pixel** — ADR 0027
§2a's 3 M stroked paths into a 240×320 target. An earlier draft said "nothing in `Limits` bounds
it", which contradicts a documented limit.

**Text extraction, by contrast, is nearly free** — 330 µs for 137 pages, two orders of magnitude
under the read-back ADR 0022 already pays. If I3 were admissible evidence it would be cheap
evidence. It is not admissible, for the reason row 21 gives.

## Finding 3 — the machinery that exists removes **four carriers**, refuses **one document**, and carries the rest — and one of the four is the one that transfers

`split` run one-way — no cuts, every page included — so every difference between input and output
is the pruning policy and the writer rather than the subsetting. That isolates exactly
redaction's question: what does this machinery remove from a page it is *keeping*?

The heading says **carriers** deliberately: on channels 15 and 16 only the carrier went, and the
page copy came through. "Removes three channels" would be a stronger sentence than the body
supports.

| # | channel | page canary before → after | carrier canary before → after | what `split` did |
|--:|---|:--:|:--:|---|
| 1 | plain-tj | yes → yes | — | carried the page copy |
| 7 | form-xobject | yes → yes | — | carried the page copy |
| 8 | type3-glyph | yes → yes | — | carried the page copy |
| 9 | actualtext | — | yes → yes | **carried the carrier** |
| 10 | structure-tree | — | yes → **no** | **removed the carrier** |
| 11 | annotation | yes → yes | yes → yes | carried both |
| 12 | acroform-field | yes → yes | yes → yes | carried both |
| 13 | optional-content | — | — | **refused the document** |
| 14 | thumbnail | yes → yes | I2 blind | carried the page copy |
| 15 | metadata | yes → yes | yes → **no** | **removed the carrier** |
| 16 | attachment | yes → yes | yes → **no** | **removed the carrier** |
| 19 | covered-by-a-rectangle | yes → yes | — | carried the page copy |
| 22 | split-content-streams | yes → yes | — | carried the page copy |
| **24** | **page-level-metadata** | yes → yes | **yes → no** | **removed the carrier** |

Channels 2–6, 17, 18, 20, 21 are marked *I2 blind* — the instrument cannot see them in either
state, so a `no` there would be the scan's silence, not a removal. **Counted as such rather than
as removals**, which is the distinction the first version of this table got wrong.

**Three of the four removals are the same removal, and it is an accident.** `/StructTreeRoot`,
`/Metadata` and `/Names` all hang off the **catalogue**, and `split`'s build route cannot reach a
destination catalogue at all — `qpdf_get_root` fails ADR 0013's caller rule. So they are not
copied. **That does not transfer to the recommendation below**, which edits in place: a redaction
that rewrites a page's streams never builds a new catalogue and therefore never drops these.
Those three measure a property of `split`'s *route*, not one redaction inherits.

**The fourth is different in kind, and it is why the recommendation strips rather than warns.**
Channel 24 puts `/Metadata` and `/PieceInfo` on the **page**, and `split` removed both — not
because it failed to copy a catalogue, but because `prune/mod.rs`'s **page-key allowlist**
(`KEPT_PAGE_KEYS`) deletes every page key outside a named set, and neither is on it. That
mechanism is **page-side, already written, already tested, and reachable from
`qpdf_get_page_n`**, so an in-place redaction can apply the identical rule to the identical
dictionary. The distinction between *the route happened to drop it* and *a rule deliberately
removed it* is the whole difference between a disclosure and a strip, and it is now measured
rather than argued.

The allowlist's reach is worth stating, because applying it to a redacted page removes more than
metadata: `/B`, `/AA`, `/Thumb`, `/StructParents` and **every key nobody enumerated** go with it.
Three of those are survival channels in their own right — `/Thumb` is channel 14 — so this is
mostly the behaviour redaction wants. It is still a behaviour change and must be stated rather
than inherited silently.

**The optional-content refusal does transfer whole.** `split` already refuses a document whose
pages reference optional content, with a message about not being able to carry the setting that
decides whether a layer is hidden. Redaction has the identical problem for a stronger reason, and
the refusal is built, worded, and has a fixture. Critically it is also **page-side**:
`prune/mod.rs` checks whether a *page* references optional content, which is reachable.

**Everything on the page is carried**, including channels 9, 11, 12 and 14. The pruning policy is
about *objects an excluded page reached*, and redaction excludes no page.

**ADR 0022's verification covers none of this.** `OutputReader`'s entire witness surface is a page
count and a per-page `/Rotate` vector.

## Finding 4 — the write verb works, and its two operands are not both permitted

**`qpdf_oh_replace_stream_data` round-trips.** Declared in the spike's own `ffi.rs` with the
signature checked against the vendored `qpdf-c.h`, it rewrote the page content of all 22
fixtures; **all 22 outputs pass `qpdf --check`**, reopen through qpdf, and load in PDFium. It is
on `engines/qpdf-trapped-functions.txt` (`via do_with_oh_void -> do_with_oh -> trap_oh_errors`)
and is declared nowhere in `core/` today. **This is the single change that makes a content-stream
redaction buildable.**

**Its cost is a range, not a precedent.** `engines/qpdf-not-exported.toml` records split's ten
object-surgery exports at **+3,340 brotli** and merge's seven at **+53,456**, and says in those
words that *"the variance is in what else gets pulled in, not in the count"* and that +53,456 is
*"the number to remember when estimating the next batch"*. An earlier draft quoted +3,340 as the
precedent, which is the one use that file tells you not to make of it. The honest estimate is
**+3,340 to +53,456**, to be re-measured rather than assumed.

**Its `/Filter` and `/DecodeParms` operands need a null object handle, and the obvious
constructor is not permitted.** `qpdf_oh_new_null` is callable — **the spike called it**, to
establish the equivalence below — but it is neither on the trapped list nor in
`engines/qpdf-untrapped-accepted.toml`, so burrow may not. Measured: asking a page dictionary for
a key it does not have, through the **trapped** `qpdf_oh_get_key`, yields a handle of type `null`
that unparses as `null`, identical to the constructor's, and the whole rewrite was done with it.

> That is a workaround and is recorded as one. The honest alternative is an argued entry in
> `qpdf-untrapped-accepted.toml`: `qpdf_oh_new_null` allocates and returns a handle, touching no
> bytes and resolving no object, which is the argument `qpdf_oh_new_integer` already carries
> there. **Getting a null by asking for a key that is not there is cleverness in the place this
> repository has least appetite for it.**

**`qpdf_oh_get_page_content_data` concatenates, and the concatenation is lossy.** A page whose
`/Contents` is an array comes back as one buffer with no stream boundaries, so writing the result
back means collapsing the array into its first stream and emptying the rest. Measured on channel
22. The output is correct and renders identically — but an operation that silently restructures a
document is one whose verification cannot then say "nothing else changed".

**What a page handle can reach, and what it cannot** — the wall, measured:

| the carrier | lives on | reachable from `qpdf_get_page_n`? |
|---|---|:--|
| `/Annots` | the page | **yes**, as an array |
| `/Thumb` | the page | **yes**, as a stream |
| `/StructTreeRoot` | the catalogue | **no** |
| `/AcroForm` | the catalogue | **no** |
| `/OCProperties` | the catalogue | **no** |
| `/Metadata` | the catalogue | **no** |
| `/Names` (`/EmbeddedFiles`) | the catalogue | **no** |

`qpdf_get_root` and `qpdf_get_trailer` are both refused by ADR 0013's caller rule.
`core/burrow-engines/src/qpdf/ffi.rs` already records both under *"Two functions burrow wanted
and may NOT have, recorded so nobody looks again"*. **This is the same wall `split` hit, and
redaction hits it harder**: `split` could answer it by dropping what it could not reach, and
redaction cannot drop a catalogue it is meant to be editing in place.

**A refusal needs a page-side signal, and four of the five do not yet have an adequate one.**
This table is what the recommendation's refusals actually rest on, and it is the weakest part of
this spike.

**Corrected 2026-09-21.** This paragraph said *three of the five* while the table below marked
only two as unmeasured, and the table was the more wrong of the two: it called the image and
path signal *measured* because the redactor already tokenises the page's content stream. That is
a page-content-stream signal, and this section's own rule is that a signal which cannot see the
thing it refuses is a bypass. **Channel 7 of this very spike measured a Form XObject's content
being invisible to a page-content-stream scan** — the same is true of an image or a path inside
one. One signal is measured; four are owed.

| refusal | page-side signal | status |
|---|---|---|
| optional content | a page's resources reference an OCG, followed **transitively over the resource graph** | **measured**, with an evade fixture: `prune/mod.rs` calls `refuse_optional_content_in` from inside `follow_resources`, and `oc-nested.pdf` plus `a_layer_one_level_down_is_refused_like_one_on_the_page` pin one level of nesting |
| an image in the region | the page's own content stream | **NOT adequate.** Past it: a `Do` inside a Form XObject, an inline `BI` image, a tiling pattern, a shading fill. Channel 20 draws directly on the page, so no evasion was measured |
| vector paths in the region | the page's own content stream | **NOT adequate.** Past it: paths inside a Form XObject, and inside a Type 3 glyph procedure. Channel 18 draws directly on the page |
| `/AcroForm` | an `/Annots` entry with `/Subtype /Widget` | **proposed, not measured.** A field whose widget is on another page walks through |
| `/StructTreeRoot` | the page's `/StructParents` | **proposed, not measured.** A `/StructElem` reaching this page's MCIDs without the page carrying `/StructParents` walks through |
| `/Metadata`, `/Names` | **none** | there is no page-side signal at all, which is why they are disclosed rather than refused |

**The difference between "refuse the document" and "refuse a page that shows this signal" is the
difference between a refusal and a bypass**, and closing it is work the M2 ADR has to do rather
than inherit from here.

**PDFium's text API is reachable and is the wrong tool anyway.** `FPDFText_*` has no references
in `core/` or `apps/` — only in this spike — and `pdfium.wasm` ships with no `EXPORTED_FUNCTIONS`
list, so reaching it on the web needs only a bridge forward and a wasm-bindgen import, **a much
thinner audit gate than qpdf's**. Three things stand against using it:

1. **It is not admissible evidence.** Channels 21 and 9 defeat it, and finding 1a shows the font
   discloses what extraction denies — the two disagree, and neither is authoritative.
2. **`tools/check-pdfium-is-render-only.sh`** makes "a visitor who lands on `/merge-pdf` and
   merges two files downloads no PDFium at all" a three-layer checked claim. **Reported, not
   resolved** — a decision about the web payload, and it belongs in an ADR.
3. **#107 is a separate question from the payload, and the existing split may already answer it.**
   Its refined diagnosis (ADR 0027, corrected 2026-09-18) is that the mechanism is *PDFium
   resident while an operation runs*. So running a text pass in the **render worker** is worth
   measuring before anything is conceded — but it cannot help here, because a text pass in the
   render worker is **by construction a sibling heap**, and **R10** requires verification to run
   on the exact bytes in the heap they are emitted from. The two constraints point opposite ways,
   and R10 is a condition of an accepted decision.

## Finding 5 — the honest ceiling: verification can assert **what the operation did**, and cannot assert that the secret is gone

Four things, in the order they bind.

**1. On five of twenty-two channels, no allowed instrument can see the secret even before the
redaction.** Channels 3, 6, 18, 20 and 21 are legible to a person and invisible to I1, I2 and I3
alike. A check that cannot establish the secret was there cannot report that it went. Three of
the five contain no text in any form; the other two are the document asserting its own meaning.

**2. #111's shape is worse here than it was for `split`, in kind rather than degree.** For a page
count, the promise is read from the operation's own reading of the input, and a wrong reading
agrees with itself. For redaction the same structure applies to *where the text is* — but the
source of that answer is `/ToUnicode` and `/ActualText`, **written by the producer of the document
being redacted**. A page count is the engine's opinion about a file. A glyph-to-character mapping
is the *file's* opinion about itself.

> Concretely: a redactor that finds "where the secret is" by decoding through the font, and then
> verifies "the secret is gone" by decoding through the same font, can be made to redact the
> wrong glyphs *and* certify the result. Channel 21 is that case, built and measured — and
> finding 1a adds the twist that the font's `cmap` tells the truth while its `/ToUnicode` lies.

**3. The residue this report did not see for its first draft was a whole channel, and the fix was
a control rather than a bigger scanner.** Channel 23 was invisible because `survives()` was a
disjunction of witnesses with no requirement that anything witness a *removal*. Control 5 is that
requirement. **The general lesson is the one `CLAUDE.md` already states one level up**: a check
that examines nothing reads as coverage, and a removal nothing observed reads as a removal.

**4. ADR 0022's frame holds; its witness does not stretch.** R8, R9 and R10 are satisfied in
shape by the existing step. What redaction adds is a content predicate, and findings 1, 1a and 2
say what it can honestly be: **not** *the secret is absent*, but *the rule I applied, applied*.

### What a verification step could truthfully assert

Every item below is decidable through the permitted API, on the evidence above.

- The output is a document, has the same page count, and the same `/Rotate` vector — ADR 0022
  unchanged.
- **No text-showing operator remains whose glyphs fall inside the region**, re-derived from the
  *emitted bytes* through a fresh parse. A statement about the operation's own rule, and the
  strongest honest one.
- **No `/ToUnicode` entry and no `/Differences` entry remains for a code the output no longer
  draws.** Both are reachable and both are editable; this closes finding 1a's first two rows.
- **Named carriers on the page are absent or empty**: `/Thumb`, and every annotation whose
  `/Rect` intersects the region.
- **The document contains no shape the operation refused to handle** — subject to finding 4's
  page-side-signal table, which is where this one is currently weakest.
- `qpdf --qdf` plus a literal scan, **as a test rather than at runtime**, exactly as
  `split_no_leak.rs` uses it.

### What it could not assert, and must not be worded as though it could

- That the secret is absent from the output. On five channels it cannot confirm it was present.
- **That the embedded font no longer describes the removed run.** The `cmap`, `post` and `glyf`
  of a subset still name its alphabet, and re-subsetting needs `hb-subset`, which is not in the
  tree. This is the residue finding 1a leaves standing after its first two rows are closed.
- That the region is visually blank. I4 measured ink 1.0000 on channel 19's black bar and 0.1361
  on channel 18's fully legible secret — **the number is higher where nothing is readable.**
- That text outside the region is untouched, beyond the page count and rotation vector.
- Anything about channels 18 and 20. There is no text object and no glyph.

---

## Recommendation

### Redaction v1 rewrites page content streams, and the font mappings that describe them

The page's content is the one place burrow can read, edit, write and re-read entirely through
trapped functions, on both platforms, with no new engine and no cross-heap step. v1 must follow
content into **Form XObjects and Type 3 `/CharProcs`** as well as the page's own streams —
channels 7 and 8 are ordinary shapes, channel 7 was fully legible after the naive pass, and the
resource-graph walk that reaches them **already exists** in `core/burrow-engines/src/prune/`.

**And it must edit the font objects**, which finding 1a added to this recommendation after the
fact. For every code the output no longer draws: drop its `/ToUnicode` `bfchar` entry (a stream,
so `qpdf_oh_replace_stream_data`) and its `/Differences` entry (a dictionary, so
`qpdf_oh_replace_key` / `qpdf_oh_remove_key`, both trapped). That closes the *verbatim* and
*glyph-name* rows outright.

**Reading `/Contents` per stream is wrong, and channel 22 is the fixture that proves it.** An
earlier draft of this recommendation said to tokenise each array element separately. Per the
specification the array **concatenates into one lexical stream**, so element-wise tokenising puts
`BT` in one parse and `ET` in another and neither parse holds a complete text object. What is
needed is **tokenise the concatenation, keep a span map back to (stream index, offset), and write
each stream from its own slice** — which also removes the silent array-collapse finding 4
measured.

> **Recommendation: build it, on `qpdf_oh_replace_stream_data`, and revisit nothing about the
> linking strategy to do so.** The write verb is trapped, the read verbs and the tokeniser ship
> today, and the size cost is +3,340 to +53,456 brotli, to be measured.

### It refuses five shapes, and the refusals are the feature

Not caveats. Refusals, in the voice ADR 0019 §4 established for `split`. **Each needs the
page-side signal finding 4's table demands; one has an adequate one and four do not yet.**

| # | refuse | because | page-side signal |
|--:|---|---|---|
| 13 | a document whose kept pages reference **optional content** | `split` already refuses this. Redaction's reason is stronger: it must not make hidden content visible | **measured** |
| 20 | a page whose region intersects an **image** | removing text from a JPEG is re-encoding it; recognising text in one is OCR | **owed** — page content stream only |
| 18 | a page whose region intersects **vector path content** | indistinguishable from a chart, a logo or a signature | **owed** — page content stream only |
| 12 | a document with an **`/AcroForm`** | the field `/V` is on the catalogue and unreachable. Channel 12's visibility is asserted, not measured — see finding 1 | **proposed only** |
| 10 | a document with a **`/StructTreeRoot`** where the region carries marked content | `/ActualText` and `/Alt` are on the catalogue and unreachable | **proposed only** |

Rows 12 and 10 are refusals *only while the catalogue is unreachable*. They are the price of ADR
0013's bar, and the ADR should say so in those words.

> **Recommendation: refuse, do not best-effort — and design the page-side trigger for each
> refusal before writing it, because a refusal keyed on a signal the operation cannot see is a
> bypass.**

### It strips every page key outside the allowlist — a strip beats a warning

**Channel 24 settles a question the earlier draft answered the wrong way.** That draft had
`/Metadata` only on the catalogue, so the only available answer was "disclose". A page can carry
its own `/Metadata` and its own `/PieceInfo`, and finding 3 measured `split`'s page-key allowlist
removing both.

So v1 reuses `KEPT_PAGE_KEYS` on every page it redacts: everything outside the named set goes.
That is an allowlist rather than a list of things to delete, which is ADR 0019's own lesson one
level down — the leak that got through `split`'s first resource filter came through `/Stash`, a
key nobody had enumerated.

| | |
|---|---|
| **stripped, measured** | page `/Metadata`, `/PieceInfo`, `/AA`, `/B`, `/StructParents`, `/Thumb`, and every key outside the allowlist |
| **the wall stops it reaching** | catalogue `/Metadata`, the document `/Info` dictionary, `/Names` → `/EmbeddedFiles`, `/AcroForm`, `/StructTreeRoot`, `/OCProperties` — reachable only through `qpdf_get_root`, which ADR 0013's caller rule refuses |

**A strip we can do beats a warning the user must act on**, and the residue is precisely the
catalogue. The disclosure below is therefore about the catalogue and nothing else, which is what
makes it specific enough to act on rather than a general caveat about "metadata".

### It handles three more channels, all reachable from the page

| # | handle | how |
|--:|---|---|
| 11 | **annotations** whose `/Rect` intersects the region | `/Annots` is on the page. Remove the annotation entirely — pruning `/Contents` and keeping the appearance is two chances to miss one |
| 14 | **the page's `/Thumb`** | on the page, a stream, and the only channel no instrument found. Remove it unconditionally on any page that was redacted |
| 9 | **`/ActualText` and `/Alt` on marked content inside the region** | in the page's own content stream. The `BDC` property list goes with the text object |

### It discloses two things it cannot reach, and the disclosure names them

**The strip above removes the page-level half.** What is left is the catalogue, and the font.

Refusing every document with catalogue XMP would refuse nearly every document a person owns,
which is not a refusal, it is not shipping. So it is disclosed — and a disclosure a person cannot
act on is a caveat. **A person looking at a redacted page cannot see metadata**, so the copy has
to say what may remain, where, and what to do.

Written in ADR 0019 §4's voice, where every limit carries its reason so a person can tell a
deliberate limit from a defect:

> **What is removed, and what is not.** The text you selected is removed from the page, from the
> file, and from the font's record of what it said. Anything the page itself carried about that
> text — its thumbnail, its accessibility text, the private notes some editors attach to a page
> — goes with it.
>
> **Three things are outside a page and are not changed: the document's title and keywords, any
> files attached to it, and the document's own XMP metadata.** Those describe the whole document
> rather than any page, and burrow has no way to reach them — so if the text you removed also
> appears in the title, in an attached file, or in the properties your editor wrote when it
> saved, it is still in this file. **Check File → Properties, and check the attachments panel.**
> If either holds what you were removing, the fix is in the program you made the document with,
> not here.
>
> **A font that was used only for the text you removed still says which letters that text used.**
> Not the words, and not the order — the set of characters. Rebuilding a font is a thing burrow
> cannot do yet, so this is stated rather than fixed.

Three sentences there carry a reason and one carries an instruction, and both are the point. *"The
title is not changed"* reads as a bug without *"those describe the whole document"*. And a person
told only that metadata "may remain" has been given something true they cannot use; a person told
**File → Properties and the attachments panel** has been given somewhere to look.

**Channel 23's residue is the second**, and it covers channels 4, 5, 6, 9, 10 and **21**.
Dropping `/ToUnicode` and `/Differences` entries closes the verbatim and glyph-name disclosures.
The subset font program itself still covers the removed run's alphabet in its own `cmap`, and
re-subsetting it needs `hb-subset` — which `docs/ROADMAP.md`'s M2 list already names and which is
not in the tree. The disclosure is weaker on a font that also sets other text, which this spike
did not measure.

**Channel 21 is handled and disclosed by the same rule as the rest, and is worth naming
separately** because it is the one where dropping the mapping is not obviously a kindness: its
`/ToUnicode` is a *lie*, so removing the entries for undrawn codes deletes a false statement
rather than a true one. The `cmap` residue is identical to channel 6's, and it is the reason the
copy says *"which letters that text used"* rather than anything about what the text said.

**This is the strongest argument in this spike for two decisions it does not take**: reopening
the catalogue question, and pulling `hb-subset` forward. Both would move a channel from
*disclosed* to *handled*.

### What else the page must say

The disclosure copy above covers what is left behind. Two more sentences cover the operation
itself, and the second is the one that is hard to write and must be written anyway.

1. burrow refuses documents it cannot redact safely rather than doing its best, and names which
   shape it found.
2. **burrow verifies that it did what it said, not that your secret is gone.** It cannot read a
   secret drawn as a picture or as shapes, and it cannot tell whether a font is telling the truth
   about what its glyphs say.

### What the verification step asserts

The list in finding 5, and `Expected` gains a variant whose rustdoc states the residue in those
words — the `add-operation` §4a requirement, with a fake engine that lies on the way back and a
mutation that deletes the check.

> **Recommendation: name the variant for what it checks, not for what a reader hopes it checks.**
> `Expected::Redacted { .. }` will be read as "the secret is gone" by everyone who has not read
> this spike. Something closer to `Expected::RegionCleared` says the true thing in the name.

### Verdict

**Build redaction v1 on page content streams and font mappings, through qpdf alone, with five
refusals, three disclosures, and a verification that asserts the rule rather than the secret.**
The one genuinely new capability it needs — `qpdf_oh_replace_stream_data` — is already trapped.
Nothing here reopens the linking gate, needs PDFium in a document worker, or needs a second
engine in a tab.

**Do not build a redaction that promises the secret is gone.** Five of the twenty-two places
measured here are invisible to every instrument this project is allowed to use; two of those are
the document asserting its own meaning; and a twenty-third was invisible to the spike's own
instruments until a control was written that required a removal to have been observed. A tool
that says "we checked" about those is worse than one that refuses them.

---

## What this changes for the operation

Recorded here so the M2 ADR argues about the right things. These belong in that ADR, not in this
spike, and **nothing is amended from inside a spike.**

1. **`qpdf_oh_replace_stream_data` needs declaring, exporting and bridging**, and the null-handle
   operand needs an argued entry in `engines/qpdf-untrapped-accepted.toml` rather than the
   missing-key workaround this spike used.
2. **Tokenise the concatenation of `/Contents`, keep a span map, write each stream from its own
   slice.** Per-stream reading is wrong; channel 22 is the fixture.
3. **`core/burrow-engines/src/pdfsyntax/` needs byte spans, string values and operator arity.**
   Its `names.rs` argues *against* an operator table — correct for an over-approximating filter,
   inverted for a rewriter. The ADR should record the inversion.
4. **Finding 3's three removals do not transfer.** They are properties of `split`'s build route.
   An in-place redaction keeps `/Metadata`, `/StructTreeRoot` and `/Names`.
5. **Every refusal needs a measured page-side trigger** — finding 4's table. **Four of five** do
   not have an adequate one yet, and a refusal keyed on an invisible signal is a bypass. Two of
   the four read the page's content stream and so miss a Form XObject; two have no signal at all.
   Tracked as [#125](https://github.com/TensorGreed/burrow/issues/125).
6. **`hb-subset` moves from a wish to a named condition.** It is what closes channel 23's residue,
   and `docs/ROADMAP.md`'s M2 list already carries it.
7. **ADR 0027's render is not a redaction preview.** Flags are zero, so an annotation still
   carrying the secret draws as a clean page — measured, channel 11, ink 0.0693 versus 0.0000.
8. **The `redaction` corpus set in `corpus/manifest.toml` is declared and empty.** These fixtures,
   or their generator, are the obvious first entry.
9. **#111 should be read again before redaction's verification is designed.** Its own amendment
   says *"redaction depends on exactly this property"*; finding 5 says the dependency is sharper.
10. **Control 5 is worth stealing.** "A removal nothing observed is not a measured removal" is a
    property any leak harness can assert, and it is the only reason this report is not still
    claiming 12 of 22.

## The record — committed, and not where you would expect

**Decided rather than deferred: `run.txt`, `run-mutated.txt` and `run-split-probe.txt` are
committed**, under `spikes/redaction-survival/measurements/`. A run you can repeat is weaker
evidence than an artifact you can diff, and this spike is its own argument for that — it shipped
two cost tables quoting a run nobody kept, and a directory of "redacted" PDFs that were the
no-op output. Neither would have survived a diff.

**Two mechanics, both deliberate.**

*Not in `results/`.* `tools/check-no-generated-files.sh` refuses any tracked path matching
`(^|/)results/`, and the probe it uses to prove that pattern is live is itself a spike results
file, from PR #37 — 24 Playwright sweep logs that reached `main` because the ignore rule lived on
a branch. **A `.gitignore` exception would not have helped**: that gate scans the tracked tree,
not `.gitignore`. Adding `spikes/**/results/run*.txt` to the ignore file would have produced a
committable path that fails CI. So the record gets a directory whose name says what it is, the
gate keeps its full reach, and `results/` stays ignored for the bulk artifacts — fixtures, PDFs,
page images — which genuinely are regenerable output.

*Written by the binary, not by a shell redirect.* `out!` sends every table to stdout and to the
record, and `main` writes the file before returning. A `>` in a documented command goes stale the
first time somebody runs the binary without it, and the file then disagrees with the run that
produced it while looking exactly as authoritative. That is the same failure as the no-op
`redacted-*.pdf` directory, and it is closed the same way: by making the artifact a product of
the run rather than of the instructions.

**What is still not committed**: the fixtures, the redacted PDFs and the page images. They are
deterministic from `make-fixtures.py` and the harness, and `*.generated.pdf` and
`**/corpus/generated/` are already precedent for keeping that kind of thing out. The *evidence
the report cites* is committed; the *inputs it was computed from* are reproducible.

## The bar — final status

- [x] Every channel has a fixture; non-vacuity passes 23 of 23, having refused a run first.
- [x] Every cell is a run, and the run is committed: `measurements/run.txt`,
      `measurements/run-mutated.txt`, `measurements/run-split-probe.txt`.
- [x] A content stream is read, rewritten and written back through trapped functions only, on
      all 23 fixtures; every output passes `qpdf --check`.
- [x] Instrument cost measured, medians of 11, on a one-page and a 137-page document, quoted
      from the committed run.
- [x] Every channel classified against `split`'s pruning, run one-way over the same fixtures.
- [x] Both verdicts recorded per channel; the human one in `eyeball-verdicts.tsv`, which the
      harness refuses to run without.
- [x] Every channel assigned to handle, refuse or disclose. **The check that says so found two
      that were not** — channels 15 and 16 had no bucket until a coverage pass over this
      document's own recommendation section. The bar caught it, not the reading.

**Spike complete.** Five controls. One refused a fixture, one refused itself, and the fifth —
written only after review found a whole channel hiding behind its absence — is the one that
changed the headline from 12 to 17.
