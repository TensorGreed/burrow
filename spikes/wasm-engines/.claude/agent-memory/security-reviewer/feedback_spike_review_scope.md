---
name: feedback-spike-review-scope
description: For spike/throwaway code, report only real vulnerabilities or flaws that would carry into production — no code-quality nits
metadata:
  type: feedback
---

When reviewing spike code (`spikes/**`), report only (a) things that are real
vulnerabilities and (b) things that would carry into a production implementation of the
option being evaluated. Skip ordinary code-quality nits, style, missing tests, and
unwrap/panic hygiene that `core/CLAUDE.md` would normally forbid — spike code explicitly
exempts itself from those.

**Why:** the spikes exist to decide an architecture (see [[project-wasm-engines-spike]]),
so the useful review output is "this option has a security property you cannot fix later",
not a lint list. The user said plainly that nits "are not interesting".

**How to apply:** for each finding in a spike, state explicitly whether it is a
spike-local artefact or a property of the option that survives into production. Prefer
findings that change the option's ranking. Reviews of `core/`, `bindings/`, and
`apps/` keep the full CLAUDE.md standard.
