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
    // THE MECHANISM SENTENCE WAS WRONG, and security review measured it. It used to say the
    // page's security policy "will refuse to load them from anywhere else" -- but `connect-src`
    // names the engine files at their ABSOLUTE origin, so the CSP permits exactly those URLs
    // and the request is sent. What blocks it is CORS: the fetch is a cross-origin one in the
    // default `cors` mode and nothing serves `Access-Control-Allow-Origin`. The outcome is the
    // same and the explanation was not, which in this repository is a bug rather than a
    // wording preference -- and this is the one page where the sentence is read by people
    // rather than by us, because the non-canonical host is permanent.
    //
    // So it says what is true at the level a reader can check: the files are somewhere else,
    // and this host is not allowed to have them.
    message:
      `This copy of Not Only PDF was built for ${builtFor} and is being served from ${servedFrom}. ` +
      `The tools will not work here: the engine files live at ${builtFor}, and a page served ` +
      `from ${servedFrom} is not allowed to load them. ` +
      `Nothing is wrong with your file and nothing has been sent anywhere. ` +
      `If you are deploying this, it needs a rebuild with BURROW_SITE=${servedFrom}.`,
  };
}

/**
 * The origin to offer as a link, or `null` if it must not be one.
 *
 * WHY THIS EXISTS AT ALL. The banner stopped being only a developer's warning when the custom
 * domain arrived: Cloudflare keeps a Pages project's `*.pages.dev` host permanently and it
 * cannot carry a redirect rule, so a real visitor can land on the non-canonical host at any
 * time. The message ends "this needs a rebuild with BURROW_SITE=…", which is advice they
 * cannot act on and which is not true for them — the site does not need rebuilding, they need
 * the other origin, and the page already knows what it is.
 *
 * WHY IT IS GUARDED. `builtFor` is this document's own `<meta>`, so it is same-origin content
 * rather than anything a person supplied — and it becomes an `href`, where `javascript:` is
 * script execution. A scheme allowlist costs one branch and removes the question rather than
 * leaving it to be re-answered by whoever next touches the stamp. `new URL` also rejects what
 * is not a URL at all.
 *
 * `.origin` rather than the value, so a stamp carrying a path or a query cannot turn this into
 * a link to somewhere else on that host.
 */
export function linkableOrigin(builtFor: string): string | null {
  try {
    const parsed = new URL(builtFor);
    return parsed.protocol === "https:" || parsed.protocol === "http:" ? parsed.origin : null;
  } catch {
    return null;
  }
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

  // A WAY FORWARD, not just a diagnosis.
  //
  // This stopped being only a developer's warning when the custom domain arrived. Cloudflare
  // keeps a Pages project's `*.pages.dev` host permanently and it cannot carry a redirect
  // rule, so a real visitor can land on the non-canonical host at any time -- and the message
  // above, which ends "this needs a rebuild with BURROW_SITE=...", is advice they cannot act
  // on and is not even true for them. The site does not need rebuilding; they need the other
  // origin, and the page already knows what it is.
  //
  // THE DECISION IS IN `linkableOrigin`, which is a pure function so it can be tested here
  // rather than only in a browser -- this file's header explains why the DOM half is left to
  // `e2e/origin-guard.spec.ts`, and a scheme allowlist is exactly the kind of rule that must
  // not be reachable only through a real deploy.
  // NOT A LINK TO THE PAGE THEY ARE ALREADY ON. `readOrigin` compares the RAW stamp to
  // `location.origin` by exact string equality, while `linkableOrigin` normalises -- so a
  // stamp of `HTTPS://EXAMPLE.COM` served from `https://example.com` is a mismatch whose
  // "working site" link points back here. A normal build cannot produce that, because
  // `build-origin.mjs` emits `URL.origin`; this is the cheap half of hardening against a
  // hand-edited stamp, and it costs one comparison.
  const canonical = linkableOrigin(verdict.builtFor);
  if (canonical && canonical !== verdict.servedFrom) {
    const go = doc.createElement("p");
    const link = doc.createElement("a");
    link.href = canonical;
    link.textContent = canonical;
    go.append(doc.createTextNode("The working site is at "), link, doc.createTextNode("."));
    banner.append(go);
  }

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
