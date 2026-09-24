---
name: residual-bound-by-uncounted-dimension
description: A "worst remaining overshoot is N ms at the X ceiling" claim — find the input dimension the ceilings do not count (bytes of whitespace/comments) and time it
metadata:
  type: feedback
---

When a doc states a residual cost "bounded by" some caps and measured "at the ceiling", check
which dimension the caps count. burrow's lexer caps operations and operands, not bytes, so the
worst lex is not at the operand ceiling.

**Why:** #175 review (2026-09-24). ADR 0029, `Limits` rustdoc and CLAUDE.md all said the geometry
walk's residual is "one lex, about 255 ms at the operand ceiling". A 1 GiB run of whitespace or
one comment lexes in ~330 ms (aarch64 release) with one operation, and scales linearly; deflate
makes it ~1 MB on disk, so "needs max_input_bytes to admit" was also false. Also measured: a
clock that moves per READ (read-counting test) cannot tell a read placed before the lex from one
after it — moving all three post-lex reads before the lex survived every geometry test.

**How to apply:** for any timing/size residual, write a 20-line example that times the unit on
the cheapest-to-encode input (whitespace, comments, one huge string) and compare to the claim.
For read-count clock tests, also mutate the read's POSITION, not just delete it.
Related: [[whole-string-witness-partial-removal]], [[mutate-the-wiring-not-the-policy]].
