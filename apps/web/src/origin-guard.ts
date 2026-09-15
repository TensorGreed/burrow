/**
 * The page checks, at run time, that it is being served from the origin it was built for.
 *
 * # Why a build can be wrong about where it lives, and why that is silent
 *
 * ADR 0014 §4 makes the origin a BUILD input. `connect-src` names the exact content-hashed
 * engine URLs and the worker bundle carries absolute URLs, because a `blob:` worker's
 * `self.location` is opaque and a relative `fetch` fails to parse before CSP is consulted. So
 * the same `dist/` served from a different origin is not a slightly-wrong build — it is a
 * build whose policy forbids every request it needs to make.
 *
 * **And the symptom is a page that looks fine until somebody chooses a file**, then reports
 * that something inside burrow failed. This has already cost an hour once, on a port change
 * from 4321 to 4322 (`apps/web/CLAUDE.md` records it), and a custom domain replacing a
 * `pages.dev` hostname is the same mistake with a longer feedback loop: the artifact works in
 * staging, is copied to the real domain, and every tool on it is dead.
 *
 * So the page says so, immediately, on every route, before anyone touches a file.
 *
 * # What it does NOT do
 *
 * It does not repair anything, and it cannot: the policy is in the markup and in `_headers`,
 * both generated, and no client-side code may loosen a CSP. The only fix is a rebuild with
 * the right `BURROW_SITE`, which is what the message says.
 *
 * It is also not a security control. A mismatch is a deployment mistake, not an attack — the
 * CSP is what refuses the cross-origin request, and it refuses whether this runs or not. This
 * turns a silent refusal into a legible one.
 */

/** What the page was built for, and what it is actually being served from. */
export interface OriginReading {
  builtFor: string | null;
  servedFrom: string;
}

export type OriginVerdict =
  | { kind: "match" }
  /** No `<meta name="burrow-built-for">`. A build that predates this guard, or a stripped one. */
  | { kind: "unknown" }
  | { kind: "mismatch"; builtFor: string; servedFrom: string; message: string };

/**
 * The verdict, as a pure function, so the interesting cases are testable without a browser.
 *
 * A SEPARATE FUNCTION FROM THE ONE THAT TOUCHES THE DOM, deliberately. The comparison is the
 * part that can be wrong in a way nobody notices — a normalisation that made every origin
 * equal would render this inert and look exactly like a page with no problem.
 */
export function readOrigin({ builtFor, servedFrom }: OriginReading): OriginVerdict {
  if (builtFor === null || builtFor === "") {
    return { kind: "unknown" };
  }
  // EXACT STRING EQUALITY, ON VALUES THAT ARE ALREADY NORMALISED. `tools/build-origin.mjs`
  // produces `URL.origin`, and `location.origin` is the same normalisation — scheme, host and
  // a port only when it is not the default. Re-normalising here would be a second opinion
  // about what an origin is, and the failure mode of a second opinion is that it is more
  // generous than the browser's, which would declare a match the CSP then refuses.
  if (builtFor === servedFrom) {
    return { kind: "match" };
  }
  return {
    kind: "mismatch",
    builtFor,
    servedFrom,
    message:
      `This copy of burrow was built for ${builtFor} and is being served from ${servedFrom}. ` +
      `The tools will not work here: the security policy in this page names the engine files ` +
      `at ${builtFor}, so the browser will refuse to load them from anywhere else. ` +
      `Nothing is wrong with your file and nothing has been sent anywhere. ` +
      `This needs a rebuild with BURROW_SITE=${servedFrom}.`,
  };
}

/** The element the build stamps its origin into. */
export const BUILT_FOR_META = "burrow-built-for";

/**
 * Read the two origins out of a document, without deciding anything about them.
 *
 * @param doc the document carrying the build's `<meta>`
 * @param loc the location it is being served from
 */
export function originReading(doc: Document, loc: { origin: string }): OriginReading {
  const meta = doc.querySelector(`meta[name="${BUILT_FOR_META}"]`);
  return {
    builtFor: meta?.getAttribute("content") ?? null,
    servedFrom: loc.origin,
  };
}

/**
 * Put the mismatch in front of the person, at the top of the page.
 *
 * NO INLINE STYLE ATTRIBUTE and no injected `<style>`: `style-src 'self'` carries no
 * `'unsafe-inline'` and no nonce (ADR 0014), so both are refused silently. The class is
 * styled in `src/styles/base.css`, which is already loaded by the layout — which also means
 * this banner is visible even though the thing it is reporting is that requests are blocked.
 *
 * `prepend` on `<body>`, so it is the first thing in reading order and the first thing a
 * screen reader reaches. `role="alert"` because it appears after load.
 */
export function announce(doc: Document, verdict: OriginVerdict): boolean {
  if (verdict.kind !== "mismatch") {
    return false;
  }
  const banner = doc.createElement("div");
  banner.className = "origin-mismatch";
  banner.setAttribute("role", "alert");

  const heading = doc.createElement("strong");
  heading.textContent = "This page is not where it was built to be.";
  const detail = doc.createElement("p");
  detail.textContent = verdict.message;

  banner.append(heading, detail);

  // `document.body` MAY NOT EXIST YET. The layout runs this from a `<script>` in `<head>`,
  // and Astro emits it as `type="module"`, which is deferred — so in practice the body is
  // there. "In practice" is not a guarantee worth betting a silent failure on, and the
  // failure would be exactly the one this guard exists to prevent: nothing visible.
  if (doc.body) {
    doc.body.prepend(banner);
  } else {
    doc.addEventListener("DOMContentLoaded", () => doc.body.prepend(banner), { once: true });
  }
  return true;
}

/** Read, decide, announce. The whole guard, for a `<script>` to call. */
export function guardOrigin(doc: Document, loc: { origin: string }): OriginVerdict {
  const verdict = readOrigin(originReading(doc, loc));
  announce(doc, verdict);
  return verdict;
}
