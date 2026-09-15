// The origin this build is FOR. One input, read in one place, consumed by everything that
// needs it.
//
// WHY A SHARED MODULE RATHER THAN TWO ENVIRONMENT READS. ADR 0014 §4 makes the origin a
// BUILD input rather than a deploy setting: `connect-src` names the exact content-hashed
// engine URLs, and the worker bundle carries absolute URLs because a `blob:` worker's
// `self.location` is opaque and a relative `fetch` fails to parse before CSP is consulted.
// Both are generated at build time, so the same origin has to reach both.
//
// It reached only one. `tools/stage-web-engines.mjs` read `BURROW_SITE`; `astro.config.mjs`
// set no `site:` at all, so `BaseLayout.astro`'s canonical link fell back to its
// `"http://localhost"` default and EVERY SHIPPED PAGE carried
// `<link rel="canonical" href="http://localhost/...">`. Two independent places to configure
// one thing, one of them silently wrong, and the wrong one invisible in every local test
// because localhost is where local tests run.
//
// So: one function, two callers, and `src/built-for-origin.test.ts` asserts the built
// artifacts agree rather than trusting that they must.
//
// THE DEFAULT IS A DEVELOPMENT DEFAULT. `http://localhost:4321` is what `pnpm dev`,
// `pnpm preview` and the e2e servers use; a deploy must pass `BURROW_SITE` explicitly, and
// `tools/check-deployable-build.sh` refuses a build that did not.

/** @typedef {{ origin: string, fromEnvironment: boolean }} BuildOrigin */

export const DEVELOPMENT_ORIGIN = "http://localhost:4321";

/**
 * The origin to build against, and whether it was asked for.
 *
 * Round-tripped through `URL` rather than used raw. This value reaches a CSP directive and
 * a `_headers` file, so a newline in it would inject arbitrary response headers on a host
 * that reads one, and a trailing slash would silently produce a policy that blocks every
 * engine. `.origin` strips path, query, fragment and any trailing slash; the constructor
 * rejects anything that is not a URL at all.
 *
 * @param {(message: string) => never} fail  how to refuse, so each caller reports in its own voice
 * @param {NodeJS.ProcessEnv} [env]
 * @returns {BuildOrigin}
 */
export function resolveBuildOrigin(fail, env = process.env) {
  const raw = env.BURROW_SITE;
  const fromEnvironment = typeof raw === "string" && raw.length > 0;
  const value = fromEnvironment ? raw : DEVELOPMENT_ORIGIN;

  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    return fail(`BURROW_SITE is not a valid URL: ${JSON.stringify(value)}`);
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return fail(`BURROW_SITE must be http(s), got ${parsed.protocol}`);
  }
  // A CREDENTIAL OR A PORT-IN-USERINFO WOULD SURVIVE `.origin` ONLY BY BEING DROPPED, which
  // is worse than refusing: the build would succeed against an origin the person did not
  // type. Refuse instead, because a deploy origin is not a place for a guess.
  if (parsed.username || parsed.password) {
    return fail(`BURROW_SITE must not carry credentials: ${JSON.stringify(value)}`);
  }
  if (parsed.pathname !== "/" || parsed.search || parsed.hash) {
    return fail(
      `BURROW_SITE must be an origin, not a URL with a path, query or fragment: ` +
        `${JSON.stringify(value)} -- did you mean ${JSON.stringify(parsed.origin)}?`,
    );
  }
  return { origin: parsed.origin, fromEnvironment };
}
