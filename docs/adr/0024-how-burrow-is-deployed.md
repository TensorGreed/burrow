# ADR 0024: how burrow is deployed

**Status:** accepted
**Date:** 2026-09-15

## Context

M1's web app has been built and tested for `http://localhost:4321` for its whole life. Nothing
in the repository said where it would be served from, how it would get there, or what would
hold the credential that put it there. Three decisions were made to answer that, and each is
the kind this repository requires a record for: they will be asked about twice, and the second
asking will be by somebody who cannot see the reasoning from the code.

## Decision 1 — Cloudflare Pages, and the host choice is load-bearing on ADR 0014

**Cloudflare Pages, deployed by direct upload from our own CI.**

The host is not interchangeable. [ADR 0014](0014-web-engine-loading-and-csp.md) §5 duplicates
the Content-Security-Policy into a `<meta>` element *because a static host may ignore
`_headers`* — and a `<meta>` CSP cannot carry `frame-ancestors`. So on a host that ignores that
file, burrow ships without clickjacking protection and without `nosniff`, and nothing in the
page can compensate.

GitHub Pages was rejected on exactly that: it ignores `_headers` entirely. Cloudflare Pages
reads it, which is why the generated file is already in its format.

**This is asserted on the live site, not assumed.** The deploy workflow ends by `curl`-ing the
deployed origin and failing if the response carries no `Content-Security-Policy` header or no
`nosniff`. A host that quietly stopped reading `_headers` would fail the deploy rather than
serve an unprotected site.

## Decision 2 — direct upload, never Cloudflare's git integration

Cloudflare Pages can build from a connected repository. **We do not use that**, and the reason
is the shape of the workflow rather than a preference about build servers.

The engines are compiled C and C++ — qpdf, zlib, libjpeg-turbo — through a pinned emsdk, and
the integrity digests in the generated CSP are taken from those bytes. Building on
infrastructure we do not control would mean the artifact's provenance is a claim by a third
party, and it would dissolve the credential split described below: there would be no separate
upload step to isolate, because the builder would *be* the deployer.

Direct upload keeps one property that matters more than convenience: **the bytes that are
served are bytes this repository's CI produced, gated, and verified, and nothing else touched
them in between.**

## Decision 3 — the triggers are the security boundary, and the build holds no credential

`.github/workflows/deploy.yml` has exactly two triggers: `workflow_dispatch`, and `push` to
`main`. There is no `pull_request` trigger and there must never be one — a `pull_request` run
is started by a branch anyone can open, and this workflow can write to the site people visit.

**It is two jobs, and the split is the point.** `build` compiles third-party C++, runs a
package manager, and produces the payload — with **no secret in its environment at all**.
`publish` holds the Cloudflare token and does four things: download the artifact, verify it,
upload it, and read back where it went. A credential that can rewrite a public website does not
belong in the environment that runs `pnpm install`.

`tools/check-deploy-workflow.py` enforces all of this rather than trusting the comments, and it
parses the YAML rather than grepping it — the first version grepped, and a security review
walked past it five ways, each of which printed `OK — the deploy workflow cannot be triggered
from outside main`. That sentence being false while printed is the failure the file exists to
prevent, so the rules now run over the parsed structure, where an allowlist cannot under-count.

### What this does NOT close, stated rather than implied

**`workflow_dispatch` runs the workflow file from the ref it is dispatched from.** Anyone with
write access can push a branch carrying a modified `deploy.yml` and dispatch it; passing `ci`
is not a precondition. Nothing in the repository can prevent that, because the check itself
would be the modified file.

What bounds it is a GitHub setting, and it is therefore recorded here rather than in a script
that cannot see it:

- the Cloudflare values are **environment secrets on `production`**, not repository secrets, so
  a workflow that does not name that environment cannot read them;
- the `production` environment's **deployment branch rule is restricted to `main`**, so a
  dispatch from any other ref fails at the `publish` job.

Without both, `environment: production` in the workflow is a label rather than a control, and
the boundary this ADR describes has a hole in it. `tools/check-deploy-workflow.py` asserts the
half it can see — that **no other workflow in the repository mentions `CLOUDFLARE_`** — because
`ci.yml` does run on `pull_request`, and a one-line edit there would read the token out if it
were a repository secret.

## Decision 4 — one origin input, checked at run time

[ADR 0014](0014-web-engine-loading-and-csp.md) §4 already made the origin a **build** input:
`connect-src` names the exact content-hashed engine URLs, and the worker bundle carries
absolute URLs because a `blob:` worker's `self.location` is opaque. A `dist/` is therefore
bound to one origin, and the same bytes served anywhere else are a site whose every tool is
dead — silently, until somebody chooses a file.

`tools/build-origin.mjs` is the single reader of `BURROW_SITE`. It feeds the CSP, `_headers`,
the worker manifest, Astro's `site:` and a `<meta name="burrow-built-for">` stamp on every
page. `apps/web/src/origin-guard.ts` compares that stamp against `location.origin` at run time
and says so, visibly, when they differ; `tool-host.ts` refuses to start the engines.

**A custom domain is a rebuild with one changed variable, not a rework.** Verified against
`https://burrow.example` before the first deploy, so that claim is measured rather than hoped.

## Consequences

- **The production origin is `https://burrow-f2s.pages.dev`, and the Cloudflare project is
  called `burrow`.** They differ, and that is not a mistake: Cloudflare generated the subdomain
  `burrow-f2s` because `burrow.pages.dev` was taken. Moving to a custom domain means changing
  `BURROW_SITE` and redeploying, because the origin is baked into the build.

  **THE PROJECT NAME WAS DERIVED FROM THE HOSTNAME AND THAT COST THE FIRST DEPLOY.** The
  reasoning was sound — Cloudflare names a project's subdomain after the project — and it is a
  convention rather than a rule. The run failed with `The Pages project "burrow-f2s" does not
  exist`, having uploaded nothing.

  It is worth recording as more than an error, because of its shape: a derivation that is
  **right for the wrong reason keeps looking right**, and the next person would have
  re-derived the same wrong answer from the same sound argument. The two are stated separately
  now — `BURROW_SITE` is a *build* input, `BURROW_PAGES_PROJECT` is a *deploy* input — and
  `tools/check-deploy-workflow.py` asserts both are set and that the upload reads the variable
  rather than restating the name. Nothing compares them, deliberately: they are allowed to
  differ, and the self-test has a case asserting a checker that refused the difference would
  be wrong.

  What actually stops a wrong project name reaching people is the read-back below, not a rule
  about names.
- **Merging to `main` deploys.** There is no separate approval step between a merged pull
  request and the live site, by design — the ruleset requires a pull request and a passing
  `ci`, and that is the gate.
- **Two GitHub settings are load-bearing and invisible to every check in this repository**:
  environment secrets on `production`, and that environment's branch rule. They are named in
  *What this does not close* above. If they are not in place, the trigger boundary is weaker
  than this document says it is.
- **The deploy asserts its own outcome, and it took two goes to ask the right question.**
  The first version asserted `wrangler`'s reported URL *equals* `BURROW_SITE`. Wrangler prints
  a **per-deployment alias** — `https://f55a5097.burrow-f2s.pages.dev` — and prints one for a
  production deploy too, so that assertion could never pass. It failed a run whose upload had
  succeeded: the site was live and the deploy was reported red.

  Two questions were being conflated, and they need different answers. *Which deployment did
  wrangler create?* Always an alias; `tools/check-deployment-url.sh` checks it belongs to the
  expected project. *Did production actually update?* Not knowable from wrangler's output at
  all — the step after it asks the live origin for its headers, which is the only honest way.

  **The match is by host LABEL, not by characters**, and that is the fourth deny-or-match rule
  in this repository where the string form was the defect: `evil-burrow-f2s.pages.dev` ends
  with the same characters as the expected host. The others were the force-push deny list, the
  `_headers` origin check using its argument as a regex, and this workflow's triggers matched
  with grep. Same shape every time — a string operation standing in for a structural
  comparison, correct on every example anybody thought to try.

  It is a script rather than inline shell for the reason `CLAUDE.md` gives about gates: an
  inline gate is one nothing can test, and this one was inline and wrong.
- **`npx --yes wrangler@4.132.0` is the one unpinned dependency closure** in either workflow,
  in the job that holds the credential. The version is pinned and npm forbids republish, but
  its transitive tree is not, and it is outside both licence gates. It is installed *before*
  the final verification so the gate is the last thing to touch `dist/`; closing it properly
  means `wrangler` as a locked devDependency, which is a dependency decision rather than a
  detail. Recorded as an accepted residual, not as a solved problem.
