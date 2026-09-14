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

**The worker is created from a `blob:`, and that is a security property, not a style.** A
dedicated worker loaded from a same-origin _script URL_ does **not** inherit the page's CSP —
it takes its policy from that script's HTTP response headers, and a static host sends none,
so it runs unpoliced. Measured in M1 PR 4a-i: a cross-origin `fetch` from inside such a
worker reached the network while every page-level CSP test passed. The worker is the only
place file bytes ever exist.

So: `tools/stage-web-engines.mjs` bundles **all** worker code into one file (which also means
one integrity digest covers the Emscripten glue, which `importScripts` could never pin), the
page fetches its source with `integrity`, and constructs the worker from a `Blob`. The worker
refuses every file operation unless `self.location.protocol === "blob:"`, so getting this
wrong fails closed. Do not change it back to `new Worker(url)`.

Three consequences you will meet:

- **No inline scripts.** `script-src 'self'` carries no `'unsafe-inline'` and no nonce, so
  an inline `<script>` is refused — the harness hit this and moved to an external file
  rather than the policy loosening. Pass data to a script through a `data-` attribute.
- **The browser does not stop exfiltration, and the CSP cannot.** CSP ignores query
  strings, so `…/qpdf.<hash>.wasm?leak=…` matches the permitted source. What closes that is
  a test asserting **zero network requests of any type** once the engines have loaded. Both
  halves are needed; neither is sufficient. `e2e/csp.spec.ts` includes a test that
  deliberately demonstrates the hole, so nobody reads the other two and concludes otherwise,
  and `e2e/zero-requests.spec.ts` is the test that closes it. **The test server's request log
  is the ground truth**, not `page.on("request")`: browser-reported network events for
  dedicated workers are not equally complete across engines, and the worker is the only place
  file bytes exist. `e2e/server.mjs` serves `dist/` and logs every request; a second origin
  logs anything that reaches it and fails the test if anything does.
- **Relative URLs do not work inside the worker.** A `blob:` worker's `self.location` is an
  opaque `blob:` URL, so `fetch("/engines/…")` fails to parse before CSP is consulted. The
  generated manifest inside the bundle carries absolute URLs, and Emscripten is handed
  already-compiled modules so its `locateFile` path never runs.

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

**The worker lifecycle is a state machine, and it lives in `src/host/worker-host.js`.**
See [ADR 0015](../../docs/adr/0015-web-worker-lifecycle.md). Five states — `idle`,
`initialising`, `busy`, `dead`, `respawning` — with every dependency injected (`spawn`, `now`,
`setTimer`, `clearTimer`), so `worker-host.test.ts` drives every transition in milliseconds
against a fake worker. Do not put lifecycle logic in a page or an island; put it there, with a
test.

`src/host/*.js` are ES modules copied verbatim into `public/host/` by
`tools/stage-web-engines.mjs` and deleted from production builds by `astro.config.mjs`. The
sources live in `src/` so vitest can import them; `tsc -p src/host/tsconfig.json` type-checks
them and runs as part of `pnpm check`.

**Engines load on first use, and the start-up bound times SILENCE rather than duration.**
See [ADR 0018](../../docs/adr/0018-when-the-engines-load.md), which settles it for every tool
page. Two things there are easy to get wrong again:

- **A start-up bound cannot be a fixed duration.** It was 60 s for the whole of start-up, with
  a comment calling that "comfortably above any measured cold load" — and on Chrome's "Slow 3G"
  profile the engines take over two minutes, so the first file a person chose was refused with
  "Something inside burrow failed" and the worker discarded _as a crash_. Three tries would
  have latched the breaker. The worker now posts `{ starting: true }` as each module lands and
  the timer restarts; what is bounded is a worker that has gone quiet.
- **`integrity` means one progress message per module, and no more is possible.** SRI makes the
  browser verify a whole response before releasing any of it, so a module's body arrives in one
  read — measured: the same file without integrity arrives in 3,545 chunks. Three finer designs
  were built and all reported two events across a 130-second download. Do not try a fourth.

**The watchdog's clock starts when the worker takes the operation, not when the page asks.**
The worker posts `{ id, ack: true }` before it begins; the page's timer starts on that ack.
Engine start-up has its own bound (`initTimeoutMs`), because a cold 6.8 MB compile must never
be charged to the file — that is the web form of the queue-time bug PR 2 fixed natively. A
start-up failure is `Internal`, **never** `LimitExceeded`.

**Operations are serialised, because the worker is.** `run()` queues; only one operation is
posted at a time. Concurrency here is not free parallelism — the worker's message loop is
single-threaded, so a second operation just sat unacked behind the first, and its start-up
timer then killed the healthy worker running the first (ADR 0015 §2a). A queued request carries
no deadline: nothing about waiting is the file's fault.

**Send the `File`/`Blob`, never a transferred `ArrayBuffer`.** Structured clone passes a Blob by
reference, so the page never holds the bytes — and, the part that matters for recovery, the
caller still holds a usable handle after its worker is killed. A transferred buffer is detached
page-side and gone, so retrying the same file would be impossible for exactly the files that
needed it.

**Never retry automatically, and the circuit breaker is why.** Three crashes in 60 seconds
opens it; every later request returns `EngineUnavailable` and nothing is spawned until
`reset()`, which a UI wires to a deliberate gesture. It counts **crashes**, not respawns — a
recycle and a user cancel are respawns, and counting those took the page offline while every
worker was healthy (measured; ADR 0015 §3). A tool page must surface `EngineUnavailable` as
something a person can act on.

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
  `e2e/recovery.spec.ts` is it, and `src/host/worker-host.test.ts` covers the transitions a
  browser will not reproduce on demand.

**A worker whose engine heap has grown too far is recycled — after the result is delivered.**
Recycling is not a failure and the caller must never see it as one. The verdict is
`reply.recycle`, computed in Rust from `Limits::max_memory_bytes`
(`core/burrow-engines/src/web/recycle.rs`), for the same reason `fatal` is: ADR 0009 forbids a
binding enforcing any part of `Limits`. The threshold and the measurements behind it are in
ADR 0015 §5-6 — ordinary corpus work sits at ~18 MiB per engine, and `xref-bomb.pdf` reaches
1.9 GiB when a caller raises the ceiling far enough to let it.

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

## Design

The visual language every tool page inherits. It landed in M1 PR A, before any tool page
existed, so that the five tools would share one system rather than five.

**The principle, and everything else follows from it: the interface reports, it does not
reassure.** This is a site whose entire claim is that your files do not leave your computer.
A design that argues that with warmth and adjectives is asking to be trusted; this one is
built to be checked. Every number shown is one we measured. Where we cannot measure
something, the page says what the limit is instead — which is the interface form of the rule
the root `CLAUDE.md` states for doc comments, where an overclaim is a bug rather than a
wording preference.

### The tokens

`src/styles/tokens.css` is the whole palette and scale; `src/styles/base.css` applies it.
Both are imported by `BaseLayout.astro`, so a page gets them by using the layout.

Six colour values, and the two coloured ones are **reserved**:

| token         | for                                                                            |
| ------------- | ------------------------------------------------------------------------------ |
| `--paper`     | the ground                                                                     |
| `--ink`       | text; never a background                                                       |
| `--ink-quiet` | secondary text                                                                 |
| `--rule`      | a hairline that separates. Decorative; never outlines a control                |
| `--edge`      | a boundary you can act on — a drop zone, an input, a button. >= 3:1            |
| `--signal`    | **machine state only**: a count we measured, a "done", a live readout          |
| `--refuse`    | **refusals and limits only**: a file we would not open, a ceiling that was hit |

**Links and buttons are ink, not `--signal`.** This is the costliest rule in the system and
the point of it. The moment a button is the signal colour, the colour means "interactive" as
well as "measured", and the one number that was actually measured stops standing out. Links
are underlined; controls have an `--edge`.

`--refuse` is its own value rather than a tint of `--signal` because the two say opposite
things, and a wrong colour on an error is a correctness bug here, not a styling one.

**Contrast is asserted, not eyeballed.** `src/styles/tokens.test.ts` parses `tokens.css`,
computes WCAG contrast for every classified token against its own theme's ground in **both**
themes, and gates on the **number of comparisons made** as well as on nothing failing — a
ratio check that compared nothing would otherwise look exactly like one that compared
everything. Every rule in it is also run against a fixture it must accept and a near-miss it
must reject, on every run. Adding a colour token means classifying it as text, boundary, or
decorative-with-a-reason; an unclassified one fails.

### Type

One family: **Atkinson Hyperlegible Next**, self-hosted, weight axis 400-700, Latin subset.
It was drawn so that characters cannot be mistaken for one another, which is the argument
this site makes about its own claims — and it is not a face anyone reaches for by default.
`apps/web/fonts.toml` records where it came from, pinned to an upstream commit, what was
done to it and with which tool version, and the digest of what ships. `src/fonts.test.ts`
holds that record to the file.

**A new face needs a `<link rel="preload">` in `BaseLayout.astro`, in the same commit.**
`tools/first-load.mjs` derives the first-load payload from the built markup and states that
it does **not** follow `url(...)` inside CSS. A font referenced only from `@font-face` is
therefore a real download the size budget cannot see, and one that can be requested after
`e2e/zero-requests.spec.ts` starts asserting silence. `tokens.test.ts` fails if any
`@font-face` has no matching preload.

Numerals are tabular everywhere, set once on `body`. A measurement that changes width while
it updates looks unreliable.

### Layout

Two tracks, left-aligned throughout. `.read` carries prose and stops at `--track-read`
(58ch); evidence — a diagram, a file list, a state line — runs to `--track-readout` and is
not constrained to a reading measure. They stack below `--bp-stack`.

### Things this design does not do

Not preferences. Each is a specific thing that makes a page look generated rather than
designed, and they are listed so nobody has to rediscover the list:

- **No cards.** Content chopped into identical rounded boxes with one radius on everything
  and the same soft grey shadow under each. There is one `--radius` and it is 2px.
- **No ALL-CAPS eyebrow labels** above headings.
- **No meta strings joined with middle dots** (`A · B · C`).
- **No `WORD — fragment` labels** with a spaced em dash.
- **No monospace face for small data labels.** `<code>` is for code.
- **No `→` appended to link or button text.** A button says what it does.
- **No numbered markers** (`01 / 02 / 03`) unless the content genuinely is a sequence. The
  merge page's file list is one, because merge order is the whole point of the tool; a list
  of features is not.
- **No accenting a single word in a headline** in a different colour or weight.
- **No entrance animations and no hover transitions on everything.** Motion answers an
  action — a disclosure opening, a state changing — or it does not happen. The
  `prefers-reduced-motion` block in `base.css` governs whatever is added later.

### What a tool page is made of

`/merge-pdf` is the first one, and these are the parts the next four inherit rather than
re-decide. Each was a decision with a reason, not a shape that happened.

- **No `client:*` directive.** Astro's hydration directives bootstrap an island with an
  **inline** script, and `script-src 'self'` carries no `'unsafe-inline'` and no nonce, so the
  browser refuses it, the island never hydrates, and the tool renders as dead markup — no
  error anyone would see except a CSP line in the console. Mount the component from a plain
  Astro `<script>` block, which Astro bundles to an external `_astro/*.js`. The cast that
  needs is explained at the call site.
- **The island imports `createWorkerHost`; it does not fetch `/host/`.** The same file is also
  staged to `public/host/` for the harness and deleted from production builds. Importing it
  means Vite bundles the lifecycle into the island's chunk, so no `/host/` URL ships and the
  deletion stays correct.
- **The engines start when a file is chosen, never on page load.** Mounting the component is a
  few kilobytes; starting the engines is 6.8 MB.
- **Every typed error becomes a sentence in a pure function**, beside the component and tested
  without a browser (`src/components/merge-messages.ts`). The input is a `kind`, an index and
  a limit name — all computed in Rust — so there is no field through which an engine's prose
  could arrive. Errors do not apologise and are never vague: each says what happened and what
  to do next.
- **A wrapper error is unwrapped once, in that function**, not in the component. `InputFailed`
  says which input failed and never what was wrong with it.
- **A page that drops something says so, and says why.** `/merge-pdf` has _"What is kept, and
  what is not"_; every tool page owes the same paragraph for whatever it does not carry over.
  For a subsetting operation the reason is not a limitation, it is the rule in
  [ADR 0019](../../docs/adr/0019-how-split-builds-its-outputs.md) §2 — an output may carry
  nothing derived from what it excluded — and the page should say that in a person's words
  rather than leaving them to think it is a missing feature. ADR 0019 §4 has the wording
  `/split-pdf` is held to.
- **The page does not re-implement a ceiling.** It sends the files and reports what the core
  refuses, so the prose and the code can be caught disagreeing. Say what happens to a large
  file in the page's own words rather than letting someone discover it.
- **The page says what it is doing while the engines load.** The first file waits for 6.8 MB —
  7 seconds on Fast 4G, 145 on Slow 3G, both measured. "counting…" for two and a half minutes
  with no explanation is the page being silent about the one thing the person wants to know.
- **A size ceiling is applied before the bytes are read.** The worker calls
  `check_input_budget` with each `Blob`'s `size`, which is the same
  `burrow_core::ops::check_total_input_bytes` that `merge` calls — not a mirror of it. Checking
  after the read is no earlier than the core, and the transport materialises roughly four times
  the payload on the way (issue #51).
- **The output goes out through `tool-delivery.ts`, and a name is captured before the first
  `await`.** Generations, the capture, and the object URLs are one shared path across the three
  islands (#69). It is shared for the reason `tool-host.ts` is: it has produced two security
  findings, both of the same class — one document's bytes offered under another document's
  name, once because `choose()` did not invalidate an in-flight operation and the name was
  re-derived afterwards, once because the request signature was read back after the awaits.
  `run.hand(bytes)` takes bytes and nothing else, so re-deriving after the await is not
  discouraged, it is unexpressible. It creates **no** URL for a stale run, because an unrevoked
  object URL holds the bytes for the life of the page.
- **Cancel is `discardWorker()`, and a cancelled operation's reply is ignored rather than
  shown.** The host fails an in-flight request with `Internal` when the worker is discarded,
  which is correct from its point of view — but a person who pressed Stop did not have
  anything go wrong, and "something inside burrow failed" is the interface lying about its own
  state. Ignore the stale reply by generation; do not suppress the failure branch, or a
  genuine failure arriving a moment late disappears with it.
- **No progress bar where there is nothing to report.** The merge is one engine call per page
  inside a worker and reports nothing until it finishes. Say so.
- **The result is a link the person activates**, not a download that starts itself. A tool that
  writes to someone's disk without being asked is doing something they did not request.
- **`EngineUnavailable` gets a deliberate gesture**, not a retry: the breaker latches on
  purpose (ADR 0015 §3).
- **The page is the subject of the safety tests, not only `/harness`.** `/harness` is deleted
  from production builds, so a console-silence or zero-requests assertion that runs only there
  says nothing about a route a person can visit. `e2e/merge-pdf.spec.ts` runs both against the
  page that ships, using the helpers in `e2e/request-log.ts` and `e2e/console-noise.ts`.
- **Serve the build on the port the CSP names** (4321). `connect-src` carries absolute engine
  URLs, so the same `dist/` on another port refuses every engine fetch — which looks exactly
  like a broken page, and cost an hour before it was recognised.

### The size budget measures the heaviest route, not the home page

`tools/first-load.mjs` weighs every landing route and budgets the largest, and
`size-budget.json` records which one that was. People arrive from a search for "merge pdf"
and land on `/merge-pdf`, which carries an island bundle the home page does not — budgeting
the home page would have budgeted the lightest route while the heaviest grew unwatched. A
route overtaking the recorded one fails the test rather than silently replacing it.

**`vitest.global-setup.ts` sets `NODE_ENV=production` explicitly**, and that is load-bearing:
vitest sets `NODE_ENV=test`, the builds inherit it, and Svelte resolves its client runtime by
export condition — so every build-output test was asserting against a build carrying the
**development** runtime, 10 KB larger than what deploys. It was invisible until the first
island shipped, because a build with no Svelte component pulls in no Svelte runtime to be
wrong about.

### Working on it

- **No inline `style` attributes, and no injected `<style>` tags.** `style-src 'self'`
  carries no `'unsafe-inline'` and no nonce (ADR 0014). Astro compiles `<style>` blocks to an
  external stylesheet, which is why the design system is imported rather than inlined.
- **`img-src 'self'` admits no `data:` URI.** A same-origin image is fine; a data-URI
  background is refused, silently.
- **Anything a page adds is part of the first load, or it fails `zero-requests.spec.ts`.**
  That test's `isPinnedArtifact` is an exact-match allowlist.
- **Any `console.log` left in an island fails `console-silence.spec.ts`**, which asserts the
  page console is empty, not merely free of file content.
- **Screenshot what you build, at 390px as well as on a desktop.** The landing page's
  diagram was an inline `<svg>` until a screenshot showed it rendering at about six points
  on a phone — text inside an SVG scales with the drawing. It is markup now, and it reflows.
  Nothing in `pnpm check`, `pnpm lint` or `pnpm test` would have caught that.

## Layout

```
src/pages/       one route per tool; index and static pages
src/layouts/     page shells (zero JS)
src/components/  Svelte islands and Astro components
src/host/        the main-thread worker lifecycle (staged to public/host/, test-only today)
src/worker/      the worker bundle's sources (concatenated into the generated bundle)
public/          self-hosted static assets
e2e/             Playwright, plus the logging test servers whose request log is ground truth
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
