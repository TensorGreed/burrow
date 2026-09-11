# apps/web

The burrow website: Astro, static output, Svelte islands. See
[ADR 0005](../../docs/adr/0005-web-stack.md) for why.

## Commands

Run from `apps/web/`:

```bash
pnpm install
pnpm dev        # dev server on :4321
pnpm build      # static output to dist/
pnpm preview    # serve dist/ locally
pnpm check      # astro check (types, Astro + Svelte)
pnpm lint       # prettier --check .
pnpm format     # prettier --write .
pnpm test       # vitest
```

```bash
pnpm e2e        # playwright, against a build with the harness route
pnpm build:harness  # that build, by hand
```

The wasm module comes from `bindings/burrow-wasm`; rebuild it from the repository root:

```bash
wasm-pack build bindings/burrow-wasm --target no-modules --out-dir pkg --release
```

**`--target no-modules`, not `--target web`.** This file said `web` until M1 PR 4a-i, and
that does not work: `--target web` emits an ES module, and the worker is a **classic**
worker, which cannot `import` one. It has to be classic because the prebuilt `pdfium.js` is
not modularised and can only be loaded by `importScripts`, which exists only there. The
knock-on is that the bridge's `#[wasm_bindgen]` imports carry no `module = "..."` attribute
and resolve from the worker's global scope, because that is the only style `no-modules`
supports.

After rebuilding, restage — the engine URLs are content-hashed and the CSP is generated
from them, so a stale manifest means the browser refuses the new module:

```bash
node tools/stage-web-engines.mjs   # or just `pnpm build`, which runs it as `prebuild`
```

## Rules specific to this app

**No third-party requests. On any page.** No CDN scripts, no Google Fonts, no analytics,
no error reporting, no embedded media from another origin. Self-host every asset. This is
not a performance preference — the privacy claim has to be verifiable by a user watching
the network tab, and a page that handles files is the worst place for a supply-chain risk.

**The CSP is generated, and it is strict enough to be inconvenient.** `default-src 'none'`
with every directive explicit, and `connect-src` naming the exact content-hashed engine
`.wasm` URLs — nothing else on the origin may be fetched. Written by
`tools/stage-web-engines.mjs`; see [ADR 0014](../../docs/adr/0014-web-engine-loading-and-csp.md).

Two consequences you will meet:

- **No inline scripts.** `script-src 'self'` carries no `'unsafe-inline'` and no nonce, so
  an inline `<script>` is refused — the harness hit this and moved to an external file
  rather than the policy loosening. Pass data to a script through a `data-` attribute.
- **The browser does not stop exfiltration, and the CSP cannot.** CSP ignores query
  strings, so `…/qpdf.<hash>.wasm?leak=…` matches the permitted source. What closes that is
  a test asserting **zero network requests of any type** once the engines have loaded. Both
  halves are needed; neither is sufficient. `e2e/csp.spec.ts` includes a test that
  deliberately demonstrates the hole, so nobody reads the other two and concludes otherwise.

**Static only.** No SSR adapter, no API routes, no server-side anything. There must be no
server that _could_ receive a file.

**One page per tool, one URL per tool.** Users arrive from search. Each tool page is
server-rendered with its own title, description, and prose. No client-side routing between
tools.

**Islands, not a SPA.** A page ships zero JavaScript until something on it needs to be
interactive. Load the wasm module lazily, when the user engages with the tool — never on
page load.

**Heavy work goes in a Web Worker.** A large PDF must not freeze the tab. Report progress
and support cancellation.

**Any `Internal` result is fatal to the worker. Terminate it and spawn a fresh one.**

This is measured behaviour, not theory — see
[spike 0001](../../docs/spikes/0001-wasm-engines.md) and
[ADR 0009](../../docs/adr/0009-web-panic-contract-and-binding-boundary.md). An earlier
version of this file described it wrongly, so the details matter:

- **A Rust panic is a _trap_, not a worker death.** On `wasm32-unknown-unknown` a panic
  ends in the `unreachable` instruction, which surfaces in JS as a catchable
  `RuntimeError: unreachable`. **No `error` event fires and the worker keeps running.**
  `catch_unwind` cannot help — there is no unwinding on this target — so the
  `burrow-ffi::guard` protection that covers Swift and Kotlin does nothing here.
- **Calls after a trap may still appear to work.** They did in the spike. That proves
  nothing: the Rust instance's invariants are broken, and it only looked fine because the
  spike's Rust held no state and the engines were in separate modules.
- **An Emscripten `abort()` inside an engine is worse, because it is quiet.** It throws a
  JS exception, arrives as an ordinary `Internal` reply, and leaves the worker alive with
  its init flags set — so nothing re-initialises. One crafted PDF silently bricks that
  engine for the rest of the session while the page keeps feeding it work.

So the contract is:

> **On any `Error::Internal`, or any exception thrown out of the wasm module, terminate
> the worker and spawn a fresh one before the next operation. Never reuse the instance.**

Nothing forces your hand — the worker survives, so there is no event to react to. Treat
the _result_ as the signal.

- `Malformed`, `LimitExceeded` and `PasswordRequired` are normal outcomes. They must
  **not** cost a worker. Only `Internal` does.
- Fail every in-flight request on a discarded worker with `Internal`. Do not retry
  automatically; a retry against a poisoned instance is worse than a visible failure.
- **Never echo the trap or abort text.** It is panic output and can carry input-derived
  bytes — the same reason the FFI guard discards the panic payload. Report the
  content-free `Internal`.
- A test must assert this: panic deliberately, assert the instance is discarded, assert
  the next operation succeeds on a fresh worker. Required by ADR 0006, not optional.

**One engine instance per worker, and memoise the init promise.** `pdfium.js` is not
modularised: its state lives in worker-global `var`s and `importScripts` does not dedupe
by URL. Load it twice in one scope and the second load rebinds every glue global while
your bridge still holds the first instance — an in-flight call then reads and writes the
_other_ instance's linear memory and dispatches through its function table. The sandbox
holds, so nothing escapes, but parses come back confidently wrong. Guard flags set after
an `await` are exactly what allows it, so memoise the **promise**, not the result.

**Suppress engine logging before opening any real file.** qpdf's default logger writes
warnings containing object numbers and byte offsets to stderr, which reaches the devtools
console — and it fires for files that parse _successfully_, not just failures. Stub
`printErr` and `print` on every engine module, and suppress at the C++ layer too.

**Never send file content anywhere**, including in logs and error messages. If an
operation fails, report the typed error from the core, not the input.

## Layout

```
src/pages/       one route per tool; index and static pages
src/layouts/     page shells (zero JS)
src/components/  Svelte islands and Astro components
public/          self-hosted static assets
```

## Conventions

- TypeScript, `astro/tsconfigs/strict`. No `any` in committed code.
- Prettier is authoritative for formatting; `pnpm lint` runs in CI.
- Components are `PascalCase.svelte` / `PascalCase.astro`; routes are `kebab-case.astro`
  and the filename is the URL.
- Commits use the `web` scope: `feat(web): add merge tool page`.

## Definition of done, in addition to the root checklist

- `pnpm check`, `pnpm lint`, `pnpm build`, and `pnpm test` all pass.
- A new tool page has its own indexable URL, title, and description.
- A Playwright test drives the tool with a real file, and the no-network assertion
  still passes.
- The page is usable by keyboard, and the drop zone has a file-input fallback.
