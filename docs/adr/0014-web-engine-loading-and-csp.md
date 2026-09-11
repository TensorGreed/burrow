# 0014. How the web loads its engines, and what the CSP actually guarantees

Date: 2026-09-11

## Status

Accepted — amends [ADR 0006](0006-wasm-linking-strategy.md) requirement 3.

Written as a separate record rather than an edit, because
[ADR 0001](0001-record-architecture-decisions.md) makes accepted ADRs append-only. ADR 0006's
decision — option 1, Emscripten engine modules bridged through JS — is unchanged, and so are
its other six requirements.

## Context

ADR 0006 requirement 3 reads:

> **Pass `Module.wasmBinary`, build `-sENVIRONMENT=web,worker`, and ship
> `connect-src 'none'`.** This makes non-negotiable #1 enforced by the browser rather than
> by re-auditing 240 KB of minified third-party JS on every PDFium bump.

The middle clause is right and is implemented. **The other two cancel each other out.**

Supplying `Module.wasmBinary` means *we* obtain the engine bytes. Obtaining them means
`fetch` or `XMLHttpRequest`, and that is precisely what `connect-src` governs. `script-src`
covers `importScripts`; `worker-src` covers `new Worker()`; neither covers fetching a `.wasm`
file. There is no directive under which a `.wasm` can be retrieved while `connect-src` is
`'none'`.

(An earlier draft of this paragraph added "in the worker too, since a dedicated worker
inherits its owner document's policy". That is **false** for a worker loaded from a script
URL, which is how this was first built. §1a is what replaced it, and the correction came from
a test rather than from re-reading the spec.)

So a build satisfying requirement 3 as written cannot load PDFium at all. The requirement
was stated from a correct intuition — the guarantee should rest on the browser rather than
on re-reading minified glue — but it names a mechanism that does not compose with the thing
it is protecting.

## Decision

### 1. The policy is narrowed, not loosened

`default-src 'none'`, every directive written out explicitly, **no cross-origin source
anywhere**, and `connect-src` naming the **exact content-hashed engine URLs**:

```
default-src 'none';
script-src 'self' 'wasm-unsafe-eval';
worker-src 'self' blob:;
connect-src <origin>/engines/pdfium.<hash>.wasm
            <origin>/engines/qpdf.<hash>.wasm
            <origin>/engines/burrow_wasm_bg.<hash>.wasm
            <origin>/engines/burrow-worker.<hash>.js;
style-src 'self'; img-src 'self'; font-src 'self';
base-uri 'none'; form-action 'none'; object-src 'none'
```

A CSP path with no trailing slash matches exactly, so this is *stricter* than
`connect-src 'self'` by a wide margin: not merely "same origin", but "these four files".
Every other URL on the origin is refused, which `e2e/csp.spec.ts` demonstrates against `/`,
`/harness`, `/engines/` and `/engines/not-an-engine.wasm`, from the page **and** from inside
the worker.

The worker bundle is a `connect-src` entry because the page fetches its **source text** —
see §1a. It is not a `script-src` entry: no script is ever loaded from that URL.

Directives are listed even where `default-src 'none'` already covers them. A reader should
not have to know which directives fall back, and a future directive that does not fall back
cannot then quietly open a hole.

`'wasm-unsafe-eval'` is required for WebAssembly compilation. It is not `'unsafe-eval'`,
which would permit `eval()` of JavaScript and is absent, as is `'unsafe-inline'`.

### 1a. The worker is loaded from a `blob:`, because a page-level CSP does not reach it

**This was found by a test, after the rest of this ADR was written and believed.**

A dedicated worker created from a same-origin **script URL** does not inherit the creating
document's CSP. It takes its policy from that script's own HTTP response headers, and a
static host sends none — so the worker ran with **no policy at all**. Measured: a
cross-origin `fetch` from inside it reached the network, while every page-level test in
`e2e/csp.spec.ts` passed. The worker is the only place file bytes ever exist; the page hands
them over and never sees them again. The protected half was the half that did not matter.

Only `blob:`, `data:` and `about:` workers inherit. So:

- The page fetches the worker's **source text** with `integrity`, from a `connect-src` entry
  naming its exact URL, wraps it in a `Blob`, and constructs the worker from that.
- `worker-src` is **`blob:` alone — not `'self' blob:`**. `blob:` is the widening this design
  needs; `'self'` is a separate capability, and it is exactly the one this section exists to
  remove: a same-origin, URL-loaded, *unpoliced* worker. It was in the list only so a test
  could construct the bad worker and watch the in-worker guard refuse it, which was the test
  dictating the policy. With it gone the browser refuses the mistake at construction **and**
  the guard still catches a worker started from a context this policy does not govern; the
  guard's refusing branch is unit-tested in `src/worker/guard.test.ts` instead.
  `script-src` deliberately does **not** get `blob:` — the worker is *constructed* from a
  Blob, never is a script *loaded* from one.
- **All worker code is in one bundled file.** The page fetches one source text, so it must
  be. That is a gain rather than a cost: `importScripts` has no integrity mechanism, so the
  three Emscripten glue files — including 160 KB of third-party PDFium glue that owns the
  heap the bridge writes to — were previously loaded entirely unverified. One digest now
  covers every line of worker code.
- The engine manifest is **generated into the bundle** rather than sent by `postMessage`:
  `pdfium.js` begins instantiating as it is parsed, so there is no moment after load and
  before instantiation at which a message could arrive.
- The URLs inside the bundle are **absolute**. A `blob:` worker's `self.location` is an
  opaque `blob:` URL with no useful base, so a relative `fetch("/engines/…")` fails to parse
  before CSP is even consulted. This is the same reason `locateFile` must never run and the
  compiled modules are handed to Emscripten directly.

### 1b. The worker fails closed

The guard **measures whether a policy is in force**, rather than inferring it from the URL
scheme. At init the worker fetches a same-origin URL that is deliberately not in
`connect-src` and requires the browser to refuse it, listening for `securitypolicyviolation`
so that "refused by policy" is distinguishable from "the request failed" — a network error is
not evidence of a policy. It refuses every file operation unless that probe was refused.

The first version checked only `self.location.protocol === "blob:"` and was called
`INHERITS_PAGE_CSP`. That name asserts more than the check established: a Blob worker inherits
the creating document's policy **whatever that is, including none**. A page that omitted the
layout, or a host that mangled the meta tag, yields a `blob:` worker with an empty policy, and
the old check returned true for it.

**The control is a dedicated resource, not a reused engine fetch.** An engine response can be
served from the HTTP cache, so offline-with-a-warm-cache would let a reused control succeed
while the probe failed for network reasons — and the guard would report "policed" with nothing
enforcing anything. The control is a few bytes staged and allowlisted alongside the engines,
fetched `cache: "no-store"`, **issued at the same moment as the probe** so both face identical
conditions.

**It does not depend on `securitypolicyviolation`.** An earlier version did, and **WebKit does
not dispatch that event in a worker**: it refused the probe, fired nothing, and the guard
concluded there was no policy and refused every operation in a browser that was enforcing it
correctly. 13 of 17 tests failed on WebKit's first local run. Using only whether requests
succeed is something every browser agrees on.

**Only a `fetch` rejection is interpreted.** Anything else — a bug in the guard — yields a
distinct `guard-error` verdict rather than being read as either answer. It still fails closed,
and the reason reaches the page **by message, never the console**. The classification is
structural (`error.name === "TypeError"`) rather than `instanceof`, because `instanceof`
compares against the current realm's constructor and fails for an error that crossed a realm
boundary — which is not hypothetical: it made every staged rejection in the unit tests read as
`guard-error`.

`src/worker/guard.test.ts` drives every state in `node:vm`, including the ones a browser
cannot produce on a correctly configured page: a `blob:` worker with no policy, a probe that
failed for a network reason, offline-with-a-warm-cache, a probe that never settles, and a
guard that throws.

**A service worker would invalidate all of this**, because it can answer either request from
its own cache or synthesise a response without the network or the policy being consulted —
and `cache: "no-store"` constrains the HTTP cache, not a service worker's `fetch` handler.
Adding one requires re-deriving this section first; recorded in ADR 0006's amendment as well,
because the temptation arrives as an offline-support feature with no obvious connection to
the CSP.

### 2. The bytes are integrity-pinned

`fetch(url, { integrity })` with a `sha384` digest, then `WebAssembly.compileStreaming`, then
handed to Emscripten through `Module.instantiateWasm`.

`connect-src` pins *where* the engines come from. SRI pins *what they are*. Without the
second, a tampered deploy or a poisoned cache could serve modified code that runs over the
user's files, from an origin the policy trusts.

**`Module.wasmBinary` is not used**, and requirement 3's naming of it is superseded.
`wasmBinary` takes an `ArrayBuffer`, so it forces a full second copy of a 5.3 MB module and
rules out a streaming compile. `instantiateWasm` accepts an already-compiled
`WebAssembly.Module`, which is strictly better and achieves the same thing requirement 3
wanted: the glue's own fetch path never runs.

### 3. The guarantee, stated accurately

Requirement 3 is replaced by:

> **No cross-origin requests, from the page or the worker — enforced by the browser.**
> **No requests at all after engine init — enforced by test.**
> **All worker code and all engine bytes integrity-pinned.**

The first line says "or the worker" because that is the part that was wrong for a while and
is easy to assume. The third says "all worker code" because that was also once narrower than
it read: the `.wasm` modules were pinned and the `.js` glue was not.

The second line is there because of a property of CSP that is easy to miss and impossible to
work around: **CSP ignores query strings.** `…/qpdf.<hash>.wasm?leak=<bytes>` matches the
permitted source and is allowed. No policy that permits the engines to load can close that,
because the permitted source is what is being abused.

So the browser enforces "nothing leaves this origin". It does **not** enforce "nothing
leaves". What closes the same-origin channel is a test asserting that once the engines have
loaded, **zero network requests of any type** occur while files are processed — fetch,
script, worker, image, font, style, navigation. That test lands in PR 4a-ii.

`e2e/csp.spec.ts` carries a test that *deliberately succeeds* in exfiltrating through a
query string, with a comment saying so. Two passing tests that show cross-origin and
same-origin requests blocked would otherwise read as a complete guarantee, and it is not
one. A reader should meet the limit at the same time as the mechanism.

### 4. A build is origin-bound

A CSP source expression requires a host: `'self'` takes no path, and there is no
origin-relative form that can carry one. So exact-path `connect-src` means the policy names
an absolute origin, and it is generated per build from `BURROW_SITE` (default
`http://localhost:4321`).

Serving a build from a different origin blocks the engine fetch. That is the policy working:
it cannot be loosened by redeploying somewhere else. It does mean the deploy origin must be
decided before a release.

### 5. `frame-ancestors` goes in the header, not the meta tag

Browsers ignore `frame-ancestors` in a `<meta>` element and log an error for it on every
page load. Including it there would be noise that teaches a reader to ignore CSP console
errors — which is worse than not having the directive. The generator emits two policies: the
meta tag's, and `public/_headers`, which carries `frame-ancestors 'none'` as well.

The meta tag is the primary mechanism, because it is enforced in every environment the site
is served from, including `pnpm preview` and Playwright. A header additionally covers
responses that are not HTML, on a host that reads `_headers`.

## Consequences

**No inline scripts, anywhere.** `script-src 'self'` with no `'unsafe-inline'` and no nonce
refuses them. The engine harness was written inline first and the browser rejected it; it
moved to an external file rather than the policy relaxing. This is a real constraint on every
future page and it is the right one — it is the difference between a policy that stops an
injected `<script>` and one that does not.

**The CSP is generated, so it cannot be edited by hand.** It is derived from the staged
artifacts by `tools/stage-web-engines.mjs`, which means adding an engine, or bumping one,
updates the policy automatically — and means a stale manifest surfaces as a blocked fetch
rather than as a silently wrong build.

**burrow's own wasm module is in `connect-src` too.** It is not an engine, but it is a
`.wasm` the worker fetches, and the directive does not care about the distinction. Leaving it
out would have meant either a looser policy or a module that cannot load.

## Alternatives considered

**Inline the engine bytes as base64, keeping `connect-src 'none'` literally.** The only
option that satisfies requirement 3 as written. Rejected: it closes the fetch channel and
nothing else — `script-src 'self'` stays open regardless, so the exfiltration path the
zero-requests test exists for is untouched. It also gives up integrity checking and streaming
compile, inflates a 6.5 MB payload by about a third, and adds a full decode copy at init. A
worse guarantee, more expensively.

**`connect-src 'self'`.** Simple, and blocks cross-origin absolutely. Rejected as
unnecessarily weak: exact paths cost nothing extra, since the generator knows the URLs, and
"these three files" is a much smaller permission than "anything on this origin".

**Put the policy only in a header.** Cleaner, and `frame-ancestors` would work. Rejected: a
static host may not read `_headers`, and `pnpm dev`, `pnpm preview` and Playwright do not
send one — so the policy would not be enforced in the environment where it is tested, which
is the environment where a regression would be caught. The generated `public/_headers` sends
it too, on a host that reads one, as defence in depth; **the guarantee does not depend on
it**, and the `<meta>` tag is what the tests exercise.

**Serve the worker script with its own CSP response header.** The other way to bring the
worker under a policy, and it keeps `worker-src 'self'` with no `blob:`. Rejected because the
protection would be invisible when absent: deploy to a host that ignores `_headers` and the
worker is silently unpoliced again, with every test still green because the test server sent
it. A `blob:` worker inherits the document's policy wherever the document is served from,
with no host configuration involved at all.
