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
`fetch` or `XMLHttpRequest`, and that is precisely what `connect-src` governs — in the worker
too, since a dedicated worker inherits its owner document's policy. `script-src` covers
`importScripts`; `worker-src` covers `new Worker()`; neither covers fetching a `.wasm` file.
There is no directive under which a `.wasm` can be retrieved while `connect-src` is `'none'`.

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
worker-src 'self';
connect-src <origin>/engines/pdfium.<hash>.wasm
            <origin>/engines/qpdf.<hash>.wasm
            <origin>/engines/burrow_wasm_bg.<hash>.wasm;
style-src 'self'; img-src 'self'; font-src 'self';
base-uri 'none'; form-action 'none'; object-src 'none'
```

A CSP path with no trailing slash matches exactly, so this is *stricter* than
`connect-src 'self'` by a wide margin: not merely "same origin", but "these three files".
Every other URL on the origin is refused, which `e2e/csp.spec.ts` demonstrates against `/`,
`/harness`, `/engines/` and `/engines/not-an-engine.wasm`.

Directives are listed even where `default-src 'none'` already covers them. A reader should
not have to know which directives fall back, and a future directive that does not fall back
cannot then quietly open a hole.

`'wasm-unsafe-eval'` is required for WebAssembly compilation. It is not `'unsafe-eval'`,
which would permit `eval()` of JavaScript and is absent, as is `'unsafe-inline'`.

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

> **No cross-origin requests — enforced by the browser.**
> **No requests at all after engine init — enforced by test.**
> **Engine bytes integrity-pinned.**

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
is the environment where a regression would be caught.
