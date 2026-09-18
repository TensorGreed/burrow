---
name: edge-injections-on-live-origin
description: Cloudflare zone features rewrite HTML at the edge on notonlypdf.com, a defect class no local gate can see; verify live before trusting a deploy gate
metadata:
  type: project
---

The production origin `https://notonlypdf.com` is a Cloudflare zone whose features can rewrite
HTML **after** the upload. Two observed: Web Analytics (beacon script, disabled by 2026-09-16)
and Email Obfuscation (rewrites `mailto:`/emails into `/cdn-cgi/l/email-protection` plus a
same-origin `/cdn-cgi/scripts/.../email-decode.min.js`) — the latter still active on `/credits/`
when checked on 2026-09-16.

**Why:** this class is invisible to every build-time and e2e gate — `dist/` does not contain it,
the e2e suite serves `dist/` from a local server, header checks read headers not bodies, and the
rewrite is conditional on a browser-like `User-Agent`, so plain `curl` sees a clean document.
A same-origin injected script is also *allowed* by `script-src 'self'`, so unlike the beacon it
executes.

**How to apply:** when reviewing anything that asserts what the live site serves, fetch the live
origin with browser headers yourself rather than reasoning about it, and expect the failure mode
to be a legitimate-looking host feature. Any such gate must say, where the failure is read, which
zone settings must stay off and that the remedy is turning the feature off, not relaxing the
check. Re-verify the status above before relying on it. See [[review-expectations]].
