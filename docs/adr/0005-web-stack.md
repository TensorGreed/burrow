# 0005. Astro with Svelte islands for the web app

Date: 2026-09-10

## Status

Accepted

## Context

The website is the first product to ship (M1) and, for most users, the only one they will
try. Two requirements shape it.

**Discoverability.** People find tools like this by searching "merge pdf". That means one
real, server-rendered, indexable page per tool, each with its own URL, title, and
content — not a single-page app with client-side routing.

**The privacy story has to be visible.** burrow's claim is that files never leave the
device. A heavy JavaScript bundle that phones home to a CDN and an analytics endpoint
undercuts that claim regardless of what the WebAssembly module does. Every third-party
script is also a supply-chain risk on the page that handles user files.

The interactive part is genuinely interactive, though: drag-and-drop, page thumbnails and
reordering, progress reporting from a long-running wasm operation in a worker, and
previews. That is not something to hand-write in vanilla DOM code across five tools.

The real payload cost is the wasm module (PDFium is large — see
[ADR 0004](0004-native-engines.md)), so the JavaScript framework's own weight should be as
close to zero as we can get it.

## Decision

We will build the web app with **[Astro](https://astro.build/)** (MIT), static-output, one
route per tool, using **[Svelte](https://svelte.dev/)** (MIT) for interactive islands.

- Astro renders each tool page to static HTML at build time. Marketing and documentation
  pages ship **zero** JavaScript.
- Interactivity is opt-in per component via Astro islands. A tool page loads its island
  and the wasm module only when the user actually engages with the tool, so the wasm
  download is not on the critical path of a page view.
- Svelte compiles components to imperative DOM updates with no framework runtime to ship,
  which keeps the JavaScript budget spent on our code rather than on a renderer.
- The wasm module runs in a **Web Worker**, so a large file cannot freeze the tab.
- **No third-party scripts, fonts, or analytics on any page that touches file content.**
  Assets are self-hosted. A strict Content-Security-Policy is part of the M1 deliverable,
  and it is the mechanism that makes the privacy claim checkable by a user with devtools
  open.

Deployment is static hosting: no server-side processing of user files exists, or can be
added later without someone noticing.

## Consequences

Tool pages are indexable and fast, and the privacy claim is inspectable — a user can
watch the network tab and see nothing leave. Static hosting means there is no server that
*could* receive a file, which is a stronger guarantee than a promise not to look.

Svelte's ecosystem is smaller than React's, so we will write more components ourselves
than we would otherwise, and fewer contributors will arrive already knowing it. Astro's
island model also means state does not flow freely between islands; shared state needs a
deliberate store, and page-level state that spans islands is awkward. This mostly affects
multi-step flows, which we should design as single islands rather than fight.

Astro is a younger framework than the alternatives, so we are accepting some churn risk
across major versions.

## Alternatives considered

**Astro with React islands.** The largest ecosystem, the most available third-party
components, and the most contributors who already know it. Rejected on payload: shipping
a framework runtime on every interactive page works against the one thing the page has to
communicate, and we would still be writing the file-handling UI ourselves.

**Astro with no framework, plain Web Components.** The smallest possible supply chain and
the most honest match to the licensing and privacy stance. Rejected as too slow to build
five tool UIs with, and hand-rolled state management for reorder-and-preview flows is
where bugs would live. Reconsider if the Svelte dependency ever becomes a problem.

**Next.js or SvelteKit.** Both excellent, both oriented around a server runtime we do not
want and will never use. Static export is possible but works against the grain, and the
existence of a server component invites someone to add a server-side path later.

**A single-page app.** Rejected outright on discoverability: one URL for all tools
forfeits the search traffic that is the main way users will find burrow.
