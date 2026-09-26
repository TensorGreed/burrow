---
name: harness-only-artifact-exact-lists
description: A bundle staged only into harness builds breaks e2e specs that pin the harness manifest exactly; run the whole e2e suite, not the new spec.
metadata:
  type: feedback
---

When a change adds an artifact staged only under `BURROW_HARNESS=1`, run the WHOLE Playwright
suite (one browser is enough), not just the spec the change added. The e2e suite runs against the
harness build, so any spec that pins the harness manifest exactly now sees the new ids.

**Why:** #137 (6f577de) added `burrow-redact-worker` as harness-only. `e2e/integrity.spec.ts`
compares `Object.keys(ENGINES)` from the harness page with `toEqual` against a fixed list, so it
went red in every browser. The new spec passed, the vitest suite passed, and production-only gates
(`check-pdfium-is-render-only.sh`, `check-redaction-not-in-base.sh`) refuse harness builds, so none
of them could see it. In the same commit `pnpm lint` also failed on a hand-edited test file. Both
would have made CI red.

**How to apply:** for web changes, always run `pnpm lint`, `pnpm check` under both staging states,
`pnpm test`, and `playwright test --project=chromium` on a separate port
(`BURROW_TEST_PORT=4391 BURROW_FOREIGN_PORT=4392`, so a parallel reviewer's server does not
collide). Treat conformance.spec's "native-outcomes.json is missing" as environmental. See
[[build-real-positive-for-absence-gates]].
