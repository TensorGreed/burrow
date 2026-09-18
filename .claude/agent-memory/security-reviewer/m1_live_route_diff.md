---
name: m1-live-route-diff
description: tools/check-live-routes.py — what the live-body diff actually covers (7 HTML routes, not the 34 assets), the silent cross-origin redirect pass, the tab-smuggled userinfo hole, and the five self-test mutations that stay green
metadata:
  type: project
---

Measured 2026-09-16 on the working tree adding `tools/check-live-routes.py` +
`tools/test-check-live-routes.sh` (written after Cloudflare Web Analytics and Email
Obfuscation both injected into live HTML and every existing gate missed them).

**Scope is narrower than the module docstring's first line.** `routes_in()` globs only
`index.html`, so the build's 7 HTML routes are diffed and its **34 non-HTML files are never
fetched** — `_astro/*.js`, the worker bundle, the engine `.wasm`, fonts, `robots.txt`,
`sitemap.xml`, `_headers`. Those scripts carry **no `integrity`** (`grep -c integrity=
apps/web/dist/index.html` → 0), so nothing covers the bytes of the code that touches user
files. Only the engine fetch is SRI-pinned.

**Silent cross-origin redirect.** `fetch()` never reads `response.geturl()`, and urllib
follows redirects by default. Reproduced: an origin that 302s every request to a different
port serving the built file → `ok /: byte-identical to the build` + `OK` + exit 0.

**Foreign-reference pass, measured misses** (all reported `NOT REPORTED`; the diff still
catches each, so these are reporting gaps): `https://origin<TAB>@evil.example/x.js`
(browsers strip the tab → evil.example; the checker's `.split()[0]`, added for `srcset`,
truncates to the origin and reads it as same-origin), unquoted `src=https://…`,
`<object data>`, `style="…url(https://…)"`, `<meta http-equiv=refresh>`, `<base href>`,
`<form action>` (only `formaction` is in the pattern), SVG `<image href>`,
protocol-relative `//host`, `rel="icon shortcut"` (order-sensitive set membership).
Cloudflare's real email-decode injection is **same-origin** (`/cdn-cgi/scripts/…`), so this
pass would not have flagged it either — the diff is the only thing that catches it.

**Self-test mutation sweep (copy both files to a temp dir; the script resolves checker and
probe from its own `$here`, so a temp dir is safe here):**

| mutation | suite |
|---|---|
| add `"canonical"` to `FETCHING_RELS` | **green** — the canonical exclusion is unbound |
| delete `Accept` + `Accept-Language` from `BROWSER_HEADERS` | **green** — only the UA is bound (the fixture keys on `"Mozilla" in UA`) |
| `f"{scheme}://{netloc}" != origin` → `not url.startswith(origin)` | **green** — the structural comparison has no test; this is the 6th string-vs-structure exposure |
| `foreign_references` → `return []` | 4 failures (binds) |
| drop the browser UA | 1 failure (binds the conditional-injection case) |

**Fixture-server race:** `start_server` kills, restarts on the same port 8734, and the
readiness probe (`curl /`) cannot tell the new server from the still-dying old one; two
refusal cases share the message `serves a body the build did not produce`, so a stale mode
can pass a case for the wrong reason.

**`REQUIRED_POST_UPLOAD_CHECKS` dict change is clean** — only `.items()` and `len()` consume
it; enforcement verified by deleting the step from `deploy.yml` (refuses, naming the new
reason). `tools/test-check-deploy-workflow.sh` plants removal of `check-deployment-url` only.

**How to apply:** before trusting any new `tools/check-*.py`, run the mutation sweep above —
green-under-mutation is the recurring failure here, not a missing rule. See
[[m1-custom-domain-origin-split]], [[m1-deploy-workflow-gate]], [[m1-ci-check-vacuity]].
