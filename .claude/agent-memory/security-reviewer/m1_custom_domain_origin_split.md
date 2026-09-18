---
name: m1-custom-domain-origin-split
description: The notonlypdf.com move — the fifth string-vs-structure regression (sitemap stray rule uses $expected as a regex), what BURROW_PAGES_HOST is and is not checked against, and what actually kills the tools on burrow-f2s.pages.dev (CORS, not CSP)
metadata:
  type: project
---

Measured 2026-09-16 on the working tree that moved `BURROW_SITE` from
`https://burrow-f2s.pages.dev` to `https://notonlypdf.com`.

**The fifth string-vs-structure defect, and it is in the new code.**
`tools/check-deployable-build.sh`'s new sitemap rule is
`stray_locs="$(printf '%s\n' "$locs" | grep -v "^$expected/")"` — `$expected` as a REGEX,
eleven lines below the comment explaining why the `_headers` rule was changed to `grep -vxF`
for exactly this reason. Reproduced end to end: a `<loc>` on `https://burrow-test/split-pdf/`
against `expected=https://burrow.test` passes, and the gate still prints
`7 URL(s), all under https://burrow.test` and `deployable to … and to nowhere else`.
The self-test's lookalike case uses `https://burrow.test.evil.example/`, which the regex does
catch, which is why the suite is green.

**Two more holes in the same rule, both reproduced:** the gate is count + prefix only, so a
sitemap that duplicates `/` and drops `/credits/` passes; and the robots rule only checks the
`Sitemap:` URL is *under* `$expected`, so `Sitemap: https://burrow.test/anything` passes while
the file actually validated is `dist/sitemap.xml`.

**`BURROW_PAGES_HOST` (the third argument to `check-deployment-url.sh`) is a new trust anchor
checked against nothing.** Validation is only "contains a dot and no scheme/path", so
`pages.dev` is accepted and then every Cloudflare Pages project's alias matches the label rule;
`evil.example` is accepted too. With the third argument present, `$expected` (the site origin)
is used for nothing but a `https://` prefix test. The label comparison itself survived every
probe I could construct (userinfo, port, uppercase, trailing dot, `#`/`?` smuggling, two-label
prefix) — it is the base that is soft, not the comparison.

**Partial inertness in the new self-test.** Deleting the whole `case "$project_host"` validation
block leaves `tools/test-check-deployment-url.sh` at 23/23 green — all three "a host, not a URL"
cases reject via the fallthrough instead. A scheme-*stripping* mutation is caught (2 failures).
`rejects_custom` checks exit status only; the sibling script's `expect_refusal` checks the
message.

**What actually kills the tools on `burrow-f2s.pages.dev`: CORS, not CSP.** `connect-src` names
absolute `https://notonlypdf.com/engines/…` URLs, so the policy *permits* the request; the
engine fetch (`tool-host.ts:217`, default `cors` mode + `integrity`) fails because nothing in
`_headers` sets `Access-Control-Allow-Origin`. The in-worker CSP probe uses
`mode: "no-cors"` and would succeed opaquely cross-origin — it never runs, because the worker
bundle is fetched the same way and never arrives. No file content reaches the network on either
host. `origin-guard.ts`'s message ("the browser will refuse to load them from anywhere else")
attributes the refusal to the CSP and is wrong in the one deployment where real users will see it.

**`linkableOrigin` is sound.** Probed `javascript:` with leading NUL/space and embedded
TAB/CR/LF, `data:`, `blob:`, `filesystem:`, protocol-relative, bare host, userinfo, IDN, uppercase
scheme — all either null or an http(s) origin. `URL` strips TAB/CR/LF before scheme parsing, so
whitespace cannot resurrect `javascript:`.

**ORIGIN cannot inject into `_headers`, robots.txt or `<loc>`** — but the thing that stops it is
`build-origin.mjs`'s host charset regex `^[A-Za-z0-9.\-:[\]]+$` (written for CSP wildcards), not
`.origin`. `https://ok&amp.example` is a valid URL whose `.origin` keeps the `&`; the regex is
what refuses it. The sitemap now depends on that regex and nothing says so.

**How to apply:** when any deny-or-match rule is added to `tools/check-*.sh`, grep the file for
`grep -v "^$` before believing the self-test. See [[m1-force-push-hook-and-deploy-gate]] and
[[m1-web-engine-path]].
