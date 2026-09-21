---
name: m2-pdfsyntax-rewriter
description: Measured facts about pdfsyntax's rewriting substrate (#128) — the inline-image EI leak reproduced in three renderers, the 7 GiB operand blowup, apply's quadratic locate, and which parts are provably clean.
metadata:
  type: project
---

Reviewed 2026-09-21 on `m2/128-content-stream-rewriter`, commit 6f92454 (ops.rs, strings.rs,
contents.rs, lexer `token_start`).

**Why:** this is the substrate M2 redaction rewrites pages with, so every defect here is a
redaction defect later. ADR 0029 §4.

**How to apply:** reuse these measurements rather than re-deriving them; re-verify before
recommending, the files move fast (see [[review-in-progress-edits]]).

## What is provably clean (do not re-litigate without new code)

- `contents::apply` matches a per-element oracle on **16,274** accepted 1- and 2-edit
  combinations over six element shapes (incl. zero-length elements, edits at separator
  offsets, adjacent edits, `(n,n)` spans). Zero diffs.
- The cross-boundary refusal is **exactly tight**: 0 accepted straddling replacements,
  0 refused non-straddling ones, over every single edit on those shapes.
- `strings::decode_string` matches an independently written PDF 32000-1 §7.3.4 decoder on
  **1,123,487** lexer-produced tokens; `encode_literal` round-trips all of them. Zero diffs.
- Operation spans **partition** the stream: over 46,332 accepted random streams, every byte
  outside a span is white space or a comment. Nothing drawable survives deleting every op.
- `MAX_NESTING` (64) is reached before the stack even on a **256 KiB** thread stack.

## The measured defects

1. **Inline-image `EI` heuristic hides drawn text.** `lexer::skip_inline_image_data` accepted
   only a whitespace-delimited `EI`; the image really ends at the length its dictionary derives.
   `q BI /W 1 /H 1 /BPC 8 /CS /G ID \x41EI\nBT … (SECRET) Tj ET\n EI Q`
   → **PDFium renders SECRET**; `operations()` reported it inside `ID`'s opaque span, with no
   `Tj` and no `Str` operand. A bare `ID` with no `BI` is the same shape.
   Closed in #128 by deriving the extent from `/L` or `/W`//`H`//`BPC`//`CS` and **refusing**
   when neither is possible; #142 tracks widening what can be derived.
2. **`operations()` amplifies ~28x with no aggregate cap.** `MAX_OPERATIONS` bounds
   operations, not operands: `("/a "*64 + "op ")` to 256 MiB → **7.16 GiB** peak RSS in 3.4 s,
   returning `Ok`. Deflates 258:1, so ~1 MB of PDF. `names.rs` caps count *and* length;
   `ops.rs` caps neither.
3. **`Contents::locate` is a linear scan, so `apply` is O(edits x parts).** 64,000 elements /
   320 kB / 128,000 edits = **3.44 s**; 4x parts is ~15x time. `pdfsyntax_contents`' own
   `MAX_PARTS = 64` makes this unreachable by the fuzzer.
4. `ops.rs` `number()` embeds up to 24 bytes of file content in `Error::Malformed`
   (numeric character class only) — against core/CLAUDE.md's absolute rule.
5. `apply` silently no-ops an edit whose span lies wholly within separator bytes
   (`(3,4)` on `["abc","de"]`). Touches no content, but `Ok` reports nothing about what landed.

## Harness notes

Probe crate pattern that worked: a standalone `[workspace]`-less crate in the scratchpad with a
path dependency on `core/burrow-engines` — `pdfsyntax` is `pub mod`, so everything is reachable
without touching the tree. `cargo` is not on PATH in the bash tool; prepend `$HOME/.cargo/bin`.
**Renderer differentials use PDFium and nothing else.** Other PDF readers are installed on this
machine and are deliberately not used, including in a scratchpad probe: this repository is
permissive-only (`CLAUDE.md` non-negotiable 2, ADR 0003), and a GPL tool cited in a finding
becomes the precedent for the next one. It is also the stronger result — PDFium is what burrow
ships and what ADR 0022's read-back reads through, so "PDFium draws it" is a statement about
burrow rather than about PDF readers in general. Reach it with
`cargo run -p burrow-engines --features native-engines --release --example dump-page-text`, or
`--example render-page` in `burrow-ops` when the question is where ink lands rather than what
text is extractable. If a case turns up that only another reader can see, bring that specific
case rather than treating it as licence.
