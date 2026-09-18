---
name: m1-web-design-tokens
description: Where the reserved-colour / token checks actually bind (base.css only), which refusal styling they miss, and the measured palette distances including the dark-theme greyscale collapse
metadata:
  type: project
---

Measured 2026-09-17 while reviewing the pages/design rework (#57 piece 3, branch
`pages-rework`, uncommitted). `src/styles/tokens-check.ts` + `tokens.test.ts` gained a
CIEDE2000 distance rule and a fill-pair contrast rule for the new `--accent`.

**What the structural defence examines: `base.css` and nothing else.**
`tokens.test.ts`'s "never fills anything with the refusal colour" and "carries a refusal
with more than its colour" both read `BASE_CSS`. The five islands and six `.astro` pages
carry their own scoped CSS and are not scanned, so the rules "`--accent` is only ever a
fill" and "`--refuse` is only ever text or a border" are unenforced exactly where a tool's
own refusal is styled. (As of this review nothing violates them — verified by
`rg 'var\(--refuse\)|var\(--accent\)' src`.)

**`.refusal` is used in ONE island.** Only `MergeTool.svelte` (4 spans). Rotate, Split,
Reorder and Compress style their refusals as `.notice` / `.choice__problem` with
`color: var(--refuse)` and no weight — so the "weight" carrier that `base.css`,
`tokens.css` and `apps/web/CLAUDE.md` all describe as one of three carriers of a refusal
covers 1 of 5 tools, while the test that asserts it is green. `.refusal` also has **no
border**, though `tokens.css` says it "carries a weight and a border".

**Measured palette numbers** (independent reimplementation of WCAG contrast + CIEDE2000,
Viénot dichromat sim; scratch script, not committed):

| pair | measured | doc claims |
|---|--:|--:|
| light signal/refuse | 56.2 | 56.2 ✓ (but `tokens-check.ts` still says "50.0", the pre-move value, in the present tense) |
| light refuse/accent | 25.2 | 25.2 ✓ |
| light accent(#b4531a)/old refuse(#8a2b1f) | 16.6 | 16.6 ✓ |
| light accent vs paper | 4.54:1 | 4.54 ✓ — but it **clears** the 4.5 body-text bar, while the comment says it does not |
| dark refuse/accent | 29.1 | not stated |
| **dark refuse/accent, greyscale** | **2.4** | not stated; only the light 13.7 is |
| **dark refuse/accent, tritanopia** | **2.45** | not stated; only the light 12.5 is |
| light signal/accent, greyscale | 5.3 | not stated |
| light refuse/accent, deuteranopia | 30.4 (my sim) | 18.0 (model unnamed, nothing in repo reproduces it) |

So the CVD/greyscale figures in the comments are **light-theme only** and the dark theme is
far worse; the form rule (fill vs text) is what actually carries it.

**How to apply:** when a colour rule is added here, ask which stylesheets the assertion
reads — `BASE_CSS` is one of eight — and whether the class it pins is the one the islands
actually use. See [[m1-web-page-registries]].
