# 0028. An accent colour, and what it cost the reserved palette

Date: 2026-09-17

## Status

Accepted. **Reverses an argument [`tokens.css`](../../apps/web/src/styles/tokens.css) made at
length**, and moves `--refuse`, which is a reserved semantic value. Both files were rewritten
rather than edited under comments that still said the opposite.

---

## Context

`tokens.css` shipped in M1 PR A with a heading of its own: *"WHY THERE IS NO ACCENT COLOUR FOR
LINKS OR BUTTONS"*. The argument was good, and it is worth restating before it is reversed:

> Because the moment a button is `--signal`, the colour means "interactive" as well as
> "measured", and a page full of buttons is a page where the one number that was actually
> measured no longer stands out.

That is true. What it does not establish is the conclusion drawn from it. It is an argument
against the **measurement** colour being spent on controls; it was applied as an argument
against **any** colour being spent on controls, and the two are not the same claim.

### What the conflation cost, measured rather than asserted

Piece 3 of [#57](https://github.com/TensorGreed/burrow/issues/57) began by building the site and
screenshotting every route at 390px and 1280px in both colour schemes — the process
`apps/web/CLAUDE.md` describes, run before a line of CSS was written. The screenshots say:

| | what the screenshot showed |
|---|---|
| the primary action | a small grey chip, the lightest element on the page, reading as **disabled** — on `/rotate-pdf`, `/split-pdf`, `/reorder-pdf` and `/compress-pdf`, where it was the browser's default button, because `MergeTool.svelte` was the only island that styled one |
| the heading above it | the heaviest thing on screen, at `--step-4` |
| the drop zone | a 1px dashed hairline |
| `/split-pdf` at 390px | the tool's own controls at about a screen and a half down a 3,518px page |

A page whose entire purpose is to be used, on which the most prominent element is a heading and
the least prominent is the action. That is the cost, and it was invisible from the CSS: every
individual rule was defensible and the composite was not.

---

## Decision

### 1. `--accent` is a third reserved value, and interaction is its reservation

| token | reserved for | may NOT be |
|---|---|---|
| `--signal` | a measurement the machine made | a link, a button, a heading, a hover state |
| `--refuse` | a refusal or a ceiling | anything merely important |
| `--accent` | **the primary action** | a measurement, a refusal, a heading, body text, a link |

**The austerity is kept exactly where trust is at stake.** `--signal` stays measurements-only;
`--refuse` stays refusals-only; **the privacy claim on the landing page stays ink.** The
sentence that says your files do not leave your computer is not decorated, because a claim in a
warm colour is a claim asking to be believed rather than checked — which is the principle this
palette was built on, and it survives.

### 2. `--refuse` moved, and the accent did not

The bar was written down before the candidates were measured: **no two semantic colours may be
closer than ΔE2000 25.** The number is derived rather than chosen — `--signal` and `--refuse`,
the pair this palette already treated as un-confusable, measure **50.0** apart in the light
theme and 49.2 in the dark one, so half of the established distance is the floor a third value
must clear.

The warm rust the brief asked for did not clear it against the brick `--refuse`:

| accent | against `--refuse` `#8a2b1f` | verdict |
|---|--:|---|
| `#b4531a` (the proposed value) | **16.6** | fails |
| `#a85a12` | 19.6 | fails |
| `#b06000` (the furthest warm value keeping 4.5:1 white-on-fill) | **23.1** | fails |
| `#7e7800` (ochre) | 40.4 | clears, and is not warm |

**Warm and far-from-a-warm-red are in tension, and no hue resolves it.** So the refusal moved
instead: `--refuse` is now `#8f1733`, a crimson. That is the value with the semantics, and
moving it is the larger change — which is why it is here and not in a commit message.

| | before | after |
|---|--:|--:|
| `--refuse`, light | `#8a2b1f` | **`#8f1733`** |
| `--refuse`, dark | `#f09a88` | **`#f5879f`** |
| ΔE accent↔refuse, light | 16.6 | **25.2** |
| ΔE accent↔refuse, dark | 12.5 | **29.1** |
| ΔE refuse↔signal, light | 50.0 | **56.2** |
| `--refuse` on paper, light | 7.78:1 | **8.14:1** |

The refusal colour got *further* from the measurement colour and *more* legible on the ground.
Nothing was traded away to buy the accent except the specific brick red.

### 3. Colour is never the only thing that says "refused"

**Simulated, not assumed.** The ΔE 25 bar is a rule about ordinary colour vision, and it says
nothing about the readers this site is otherwise careful about. Accent against refusal, under
the three common colour-vision deficiencies and in greyscale:

| | light | dark |
|---|--:|--:|
| ordinary vision | 25.2 | 29.1 |
| protanopia | 23.9 | 26.8 |
| deuteranopia | **18.0** | **17.7** |
| tritanopia | **12.5** | **5.1** |
| greyscale | **13.7** | **2.4** |

**They collapse**, and in the dark theme they become the same colour. That is not a defect in
the hue and no other hue fixes it: a warm fill and a warm-red text are one channel apart, and
that channel is the one being removed.

So the answer is not a different hue. **A refusal is carried by three things, only one of which
is the colour:**

1. **The word.** Every refusal on this site says what it refused, in a sentence. That was
   already true and it is now the load-bearing part rather than a nicety.
2. **Weight, or a bar.** `.refusal` is an inline span inside a sentence and is bold. A block
   refusal — a whole `role="alert"` paragraph — gets a 2px bar instead, because a paragraph
   set bold is not a design. **That distinction exists because the first version of this
   defence did not.** `.refusal` is used by `MergeTool.svelte` and by nothing else: the other
   four islands render refusals as `.notice` and `.choice__problem`, so a carrier documented
   as covering "a refusal" covered one island in five, while a test reading `base.css` passed.
   Found by security review, and it is the same shape as `.visually-hidden` living in one
   component — a rule that looks site-wide and is not.
3. **Form, which is the structural half.** **`--accent` is only ever a FILL and `--refuse` is
   only ever TEXT or a border.** A filled control and a bold phrase are not confusable in
   greyscale, whatever their hues do. `tokens.test.ts` asserts every half — that the inline
   refusal declares `--weight-strong` specifically rather than any `font-weight` at all, that
   the block refusal declares its bar, and that nothing is filled with `--refuse` — across
   **eleven stylesheets** rather than `base.css` alone. Each of those three was weaker in the
   first version and each survived a mutation until it was tightened.

### 4. What is checked, and what each check is for

`src/styles/tokens-check.ts` and `tokens.test.ts`, every rule with a fixture it must accept and
a near-miss it must reject on every run:

| rule | asks |
|---|---|
| `checkContrast` | can this be read, against the ground of its own theme |
| `checkFillPairs` | can `--accent-ink` be read **on the `--accent` fill** — the pair, not the token, because near-white on the paper is 1.06:1 and a rule that compared it there would fail a correct palette |
| `checkSemanticDistance` | can any two reserved colours be **told apart** — a different question from contrast, with a different answer |
| the `.refusal` rules | does a refusal survive with no colour at all |

Each reports what it examined and gates on the expected count: six distance comparisons, two
fill pairs, nine contrast comparisons per the classified lists.

---

## Consequences

**The wordmark is a brand, not a heading.** Same four words, no colour, no mark: "Not Only" at
the body weight and "PDF" at the strong one. `apps/web/CLAUDE.md` forbids *"accenting a single
word in a headline in a different colour or weight"*, and **that rule stands** — it is about
headlines, where weight-accenting a word substitutes for saying which word matters. A wordmark
is not a sentence. The exception is written into the rule rather than taken quietly. **No logo
mark yet**: a mark drawn against a palette that is still moving is a mark drawn twice.

**The long tool-page sections are `<details>`, and not one word was cut.** Every sentence is
still in the served HTML, so nothing is lost for a reader who wants it or for a search engine
that indexes it. `page` grew 333 brotli bytes rather than shrinking, and that is recorded in
`size-budget.json` as what it is.

**`/how-it-works` is the one new route.** It is not a per-tool detail page: it is the essay and
the boundary diagram that already existed inline on the landing page, moved whole so the tools
could have the front page.

**Five islands lost their duplicated control styles**, and two of those duplicates were bugs
rather than repetition. `.visually-hidden` was defined in `MergeTool.svelte` and used by four
other components, where Svelte's scoping meant it did nothing: their "visually hidden" tool
headings and `aria-live` regions were plainly visible, and their file inputs were not hidden at
all — which is why `/rotate-pdf` showed a "Turn your pages" heading and a raw **Choose File**
button. `.drop:focus-within` was in the same position, so four drop zones had no focus ring.
Both moved to `base.css` and now ship once for five pages.

**And making that rule real appeared to break WebKit — it did not, and this paragraph is the
correction.** With `.visually-hidden` live, a split on `/split-pdf` stalled until its 45-second
budget expired and its output never arrived. Removing the class from the `role="status"` region
made it stop, restoring it brought it back, and it was recorded here as cause and effect.

**It was not.** Measured afterwards at four WebKit suite runs per tree, the same failure happens
**once in four runs at [`e40a3a5`](https://github.com/TensorGreed/burrow/commit/e40a3a5)** — the
commit before this ADR — where that class was inert in those islands and could not have been
involved. Two clean runs after the change were what a one-in-four failure looks like three times
out of four. **The earlier conclusion rested on three samples against a defect that fails one run
in four, and stating it as settled was the error**, not the hypothesis.

What is actually there is older and is not about this rule: an operation on the qpdf worker path
whose output silently never arrives, on `/split-pdf` and `/rotate-pdf` alike.
[#107](https://github.com/TensorGreed/burrow/issues/107) carries the rate and the evidence and
has been rewritten to say so.

Two of the three inert uses are fixed and stay fixed — the tool heading and the file input are
genuinely hidden now. The `role="status"` region keeps the visibility it has had all along,
which is the status quo and no regression, because nothing measured here justifies moving it in
either direction. Hiding it is the correct behaviour and wants its own change, with #107
understood first.

**The thumbnail strip is untouched.** It works and it looks right; this piece preserves its
behaviour rather than redesigning it, and the only thing that reaches it is the site-wide button
style.

**The section titles stayed headings.** `<summary>` accepts one heading element, so each is
`<summary><h2>…</h2></summary>`. The first version replaced the `<h2>` with the `<summary>`,
which took nine sections out of five pages' document outlines — `/merge-pdf` had no content
`<h2>` left — while looking identical on screen. "Not one word was cut" was true and not
sufficient, and both reviews said so independently.

**One disclosure opens by default, and only one.** `/split-pdf`'s *"What is kept, and what is
not"* is `<details open>`, because what is inside it is the explanation of why a part cannot
carry data belonging to the pages it excluded —
[ADR 0019](0019-how-split-builds-its-outputs.md) §4 names the wording that page is held to, and
that promise lifted split's hold. A promise somebody has to click to find is a weaker promise
than the one that ADR describes.

---

## Alternatives considered

**Keep the brick `--refuse` and accept ΔE 23.1.** Rejected: it requires the bar to become 20
after the numbers were seen, which is the move this repository treats as a bug when a doc
comment makes it.

**An ochre accent at ΔE 40.4.** Clears everything and is not warm. Rejected on the brief, and
worth recording because it was the only candidate that made the rule comfortable.

**No accent at all; fix the hierarchy with weight and size.** This was the status quo's own
answer and it is what shipped: the button already had an `--edge` border and it still read as
disabled beside a `--step-4` heading. Size and weight alone did not do it.

**Distinguish the refusal with an icon.** Rejected for now. It is a real option and it is the
next one to reach for if the word and the weight prove insufficient; adding a glyph system to a
palette that has no glyphs is a larger change than moving one hex value.
