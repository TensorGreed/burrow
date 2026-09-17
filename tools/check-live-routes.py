#!/usr/bin/env python3
"""The bytes the live origin serves are the bytes we built, and nothing else.

WHY THIS EXISTS, AND WHY IT IS NOT A THIRD-PARTY-SCRIPT CHECK.

Cloudflare Web Analytics injected `static.cloudflareinsights.com/beacon.min.js` into every
HTML response on the custom domain, at the edge, after the upload. Every gate we had missed
it and each for a different reason:

  * `tools/check-deployable-build.sh` reads `dist/`, which is correct and was never wrong --
    the injection does not exist there;
  * `apps/web/e2e/zero-requests.spec.ts` runs against a LOCAL server serving `dist/`, so the
    edge is not in the picture at all;
  * the deploy's live step read response HEADERS, and the injection is in the BODY;
  * plain `curl` gets clean HTML. The rewrite is conditional on the request looking like a
    browser, so the obvious manual check says everything is fine.

The CSP blocked the script every time, so nothing executed and no bytes reached anybody. What
it broke is the *verifiable* half of non-negotiable #1 -- "the privacy claim has to be
verifiable by a user watching the network tab" -- and a person watching that tab saw a blocked
request to an analytics host on a site that says there are none.

THE RULE IS THE DIFF, NOT THE BEACON. A check for `cloudflareinsights` would catch exactly the
thing that already happened and nothing else; the next edge feature will have a different name.
So the question this asks is the general one: **does anything the origin serves differ, in any
way, from the file the build produced?** Every file in the build, not only its HTML. Any
difference is reported with the diff, and the foreign-reference pass is a *second* pass that
names what kind of difference it is, so a failure reads as a finding rather than a byte count.

WHAT MAKES THIS GO RED, AND WHAT TO DO ABOUT IT.

Transport is not the document: this sends no `Accept-Encoding`, so bodies arrive as identity
bytes, and compression, TLS, ETags and connection reuse cannot move them. What CAN move them is
a host FEATURE, and Cloudflare offers several that rewrite HTML -- all documented, all
legitimate, any of which turns this step red with a byte diff:

  * **Email Obfuscation** (Scrape Shield). Rewrites `mailto:` and bare addresses into
    `/cdn-cgi/l/email-protection` links and injects `email-decode.min.js`. It was ON when this
    check was written, altering author emails inside the LICENCE ATTRIBUTION NOTICES on
    `/credits/` -- a page that exists to reproduce those notices.
  * **Web Analytics / Browser Insights.** Injects a beacon script. This is the defect that
    prompted the check.
  * **Rocket Loader**, **Auto Minify**, **Automatic Platform Optimization**, and Bot Fight
    Mode's `/cdn-cgi/challenge-platform/` script. All rewrite HTML.

**The remedy is to turn the feature off, not to relax this check.** A build whose bytes the
operator cannot predict is a build nobody can verify, which is the claim this whole site makes.

AND NOTE WHEN IT FIRES. This runs AFTER the upload, because the live origin is the only place
the answer exists -- so a red run means the modified site is ALREADY PUBLIC. It is a detector,
not a gate, and the thing it protects is the next deploy plus your knowledge of the current one.

WHAT IT DOES NOT COVER, stated rather than implied: a route served live that is absent from the
build is undetectable here by construction -- this walks the build and asks the origin about
each file, so it can only see what it brought a copy of.

It runs AFTER the upload, against the live origin, because that is the only place the answer
exists. `dist/` cannot know what an edge will add to it.

`dist` MUST BE THE ARTIFACT THAT WAS UPLOADED, not a fresh local build. Two builds of the
same commit on different machines produce different engine content hashes -- the URLs are the
same length, so the byte counts match and only the characters differ -- and every route then
reports a difference that is nothing to do with the edge. In the publish job this is the
downloaded artifact, so they agree by construction. Run it by hand against a local build and
expect that noise; the signal is a difference in the DOCUMENT, and the diff is printed so the
two are told apart by looking.

Usage:
    tools/check-live-routes.py <origin> <dist>
Self-test:
    tools/test-check-live-routes.sh
"""

from __future__ import annotations

import difflib
import os
import pathlib
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

#: A request that looks like a person's browser.
#:
#: NOT A COSMETIC DETAIL -- it is the whole reason this check sees anything. Cloudflare's HTML
#: rewriter is conditional: `curl`'s default request gets the clean document and a browser gets
#: the injected one. A checker that fetched with the default `User-Agent` would have passed
#: over the live defect it was written for, printing OK, which is the failure mode this
#: repository calls "a check that silently examines nothing".
BROWSER_HEADERS = {
    "User-Agent": (
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/140.0.0.0 Safari/537.36"
    ),
    "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
    "Accept-Language": "en-GB,en;q=0.9",
}

#: Attributes the browser fetches BY ITSELF, with no gesture from the person.
#:
#: A HYPERLINK IS NOT A REQUEST, and conflating the two made the first version of this check
#: refuse the real site: it flagged the masthead's github.com link and the twelve licence URLs
#: on `/credits/`, all of which are ordinary outbound links that fetch nothing until somebody
#: clicks them. Non-negotiable #1 is about requests -- "no code path that touches user file
#: content may make a network call", and the verifiability clause is about what a person sees
#: in the network tab on load. So `<a href>` is deliberately absent here, and so is
#: `<link rel="canonical">`, which is metadata rather than a subresource.
FETCHED_ATTRIBUTES = re.compile(
    r"""\b(?:src|srcset|poster|formaction)\s*=\s*["']([^"']+)["']""",
    re.IGNORECASE,
)

#: `<link>` fetches only for some `rel` values. `canonical`, `alternate` and `author` do not.
LINK_ELEMENT = re.compile(r"""<link\b([^>]*)>""", re.IGNORECASE)
LINK_REL = re.compile(r"""\brel\s*=\s*["']?([^"'>]+)""", re.IGNORECASE)
LINK_HREF = re.compile(r"""\bhref\s*=\s*["']([^"']+)["']""", re.IGNORECASE)
#: `rel` IS A SPACE-SEPARATED LIST, and matching it whole misses the multi-valued spelling:
#: `rel="preload stylesheet"` fetches and was not flagged, while `rel="Stylesheet"` was. The
#: set is intersected with the value's words below, which also makes the old `"shortcut icon"`
#: entry redundant -- it was in the set only because somebody met that one pair.
FETCHING_RELS = {
    "stylesheet",
    "preload",
    "modulepreload",
    "prefetch",
    "preconnect",
    "dns-prefetch",
    "icon",
    "apple-touch-icon",
    "manifest",
    "prerender",
}

#: A URL inside a <script> or <style> body, where no attribute pattern would find it -- an
#: injected inline script that calls out is still a call out.
INLINE_BLOCK = re.compile(r"""<(script|style)\b[^>]*>(.*?)</\1>""", re.IGNORECASE | re.DOTALL)
ABSOLUTE_URL = re.compile(r"""https?://[^\s"'<>()]+""", re.IGNORECASE)


#: Files the HOST consumes as configuration and never serves as themselves.
#:
#: `_headers` WAS MEASURED, ON A REAL DEPLOY THAT THIS CHECK FAILED. It is how the
#: response-header policy reaches Cloudflare Pages; asking the origin for it returns **HTTP
#: 200** with the site's HTML, not a 404 -- so the diff compared a 618-byte config file
#: against a 6,217-byte web page and reported the origin as serving something the build did
#: not produce. It was right that the bytes differ and wrong about what that means. (The
#: status code is what misled the author beforehand: `curl -o /dev/null -w '%{http_code}'`
#: says 200 and says nothing about the body.)
#:
#: THE OTHER THREE ARE ANTICIPATED, NOT OBSERVED. `_redirects`, `_routes.json` and
#: `_worker.js` are consumed by the same mechanism per Cloudflare's documentation; none has
#: ever appeared in this build, so none has been seen on this origin. They are listed so that
#: adding one does not produce the same misleading failure, and the distinction is drawn
#: because a comment claiming four measurements when it has one is an overclaim.
#:
#: WHAT THE EXCLUSION LEAVES UNDETECTABLE, AND WHAT COVERS IT INSTEAD: this check can no
#: longer notice `_headers` failing to reach the host or being served wrongly. That is not
#: uncovered -- the step above it in `deploy.yml` reads the CSP and `X-Content-Type-Options`
#: back from the live origin as real response HEADERS, which is the EFFECT of the file and a
#: stronger thing to assert than its bytes.
#:
#: Excluded BY NAME and only at the ROOT: a page legitimately called `_headers` in a
#: subdirectory is a document. The exclusion is printed on every run, because a file quietly
#: dropped from a comparison is how a check stops examining things without anybody noticing.
PLATFORM_CONFIG = frozenset({"_headers", "_redirects", "_routes.json", "_worker.js"})


class Refused(Exception):
    """A rule failed. The message is the reason."""


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)


def fetch(url: str) -> bytes:
    request = urllib.request.Request(url, headers=BROWSER_HEADERS)
    # `S310` is about unvalidated schemes; `main` accepts only http(s) and refuses anything
    # else before this is reached. (It accepts http:// as well as https:// because the
    # self-test serves over a loopback socket.)
    with urllib.request.urlopen(request, timeout=30) as response:  # noqa: S310
        # A REDIRECT IS A DIFFERENT DOCUMENT, and `urlopen` follows them silently. This deploy
        # has TWO live origins serving the same bytes by design, so "the edge answered with a
        # redirect to the other one" is a reachable state rather than a hypothetical -- and the
        # check would then fetch that host, find it byte-identical, and print OK over a route
        # that does not serve what we think it serves.
        if response.url != url:
            raise Refused(f"{url} redirected to {response.url}; that is a different document")
        if response.status != 200:
            raise Refused(f"{url} returned {response.status}")
        return response.read()


def files_in(dist: pathlib.Path) -> list[tuple[str, pathlib.Path]]:
    """EVERY file in the build, as (url path, file) -- not just the HTML.

    THE FIRST VERSION CHECKED THE SEVEN `index.html` FILES AND NOTHING ELSE, while the build
    has 41 files. Security review named what that left open, and it is the hole this check was
    written to close, one layer down: `_astro/*.js` carries **no** `integrity` attribute --
    only the ENGINE fetch is SRI-pinned, inside `tool-host.ts` -- so the island JavaScript that
    drives a person's file through the worker was covered by no pin and no diff. An edge
    serving a modified `_astro/*.js` passed this gate, passed `check-deployable-build.sh`
    (which reads `dist/`), and passed the e2e suite (which serves `dist/` from a local server).
    Seven of forty-one is exactly the "4 of 15 reads as success" shape, in a check written to
    stop that.

    So: everything. `index.html` is addressed by its DIRECTORY url, because that is what a
    person visits and what the canonical link names; every other file by its own path.
    """
    found = []
    for path in sorted(dist.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(dist).as_posix()
        if relative in PLATFORM_CONFIG:
            continue
        # A CONSUMED-BY-HOST *DIRECTORY* IS NOT THE SAME SHAPE and must not be waved through.
        # `_worker.js/` (advanced mode) and `functions/` put their children back into the
        # compare loop as `_worker.js/index.js`, which would fail with exactly the misleading
        # message this exclusion exists to remove. Widening the exclusion to path PREFIXES
        # would instead create a real blind spot, so this refuses loudly and leaves the
        # decision to a person.
        if relative.split("/")[0] in PLATFORM_CONFIG:
            raise Refused(
                f"{relative} is inside `{relative.split('/')[0]}`, which the host consumes "
                f"rather than serves. This check does not know what to compare it against; "
                f"decide deliberately rather than letting it fail as a served file."
            )
        if relative.endswith("index.html"):
            directory = relative[: -len("index.html")]
            found.append((f"/{directory}", path))
        else:
            found.append((f"/{relative}", path))
    return found


def differences(built: bytes, served: bytes) -> list[str]:
    """A unified diff of the two documents, or an empty list when they are identical.

    Decoded as text for the diff only. A body that is not valid UTF-8 is a difference in
    itself and is reported as one rather than crashing the check.
    """
    if built == served:
        return []
    try:
        a = built.decode("utf-8").splitlines()
        b = served.decode("utf-8").splitlines()
    except UnicodeDecodeError:
        return [f"the served body is not valid UTF-8 ({len(served)} bytes)"]
    return [
        line
        for line in difflib.unified_diff(a, b, fromfile="built", tofile="served", lineterm="", n=1)
    ]


def foreign_references(body: bytes, origin: str) -> list[str]:
    """Every reference the BROWSER WILL FETCH that is not on this origin.

    THE SECOND PASS. The diff above already fails on any injection; this names what kind it is,
    so the report says "a script from another origin" rather than "line 3 differs". It also
    stands on its own: were the diff ever relaxed, this still refuses the class of thing that
    made the diff necessary.

    Hyperlinks are NOT included -- see `FETCHED_ATTRIBUTES`. Flagging them made the first
    version refuse the real site over its own masthead link and its licence citations, which
    would have taught whoever met it to stop believing the check.
    """
    text = body.decode("utf-8", errors="replace")
    candidates: set[str] = set(FETCHED_ATTRIBUTES.findall(text))

    for attributes in LINK_ELEMENT.findall(text):
        match = LINK_REL.search(attributes)
        rel_words = set((match.group(1) if match else "").lower().split())
        if rel_words & FETCHING_RELS:
            href = LINK_HREF.search(attributes)
            if href:
                candidates.add(href.group(1))

    for _tag, inner in INLINE_BLOCK.findall(text):
        candidates.update(ABSOLUTE_URL.findall(inner))

    found: list[str] = []
    for candidate in candidates:
        # `srcset` is a comma-separated list of "url descriptor" pairs.
        # TAB, CR AND LF COME OUT FIRST, BEFORE ANY SPLITTING, and the ORDER is the whole point.
        # Browsers strip those characters before parsing a URL, so
        # `https://good.example<TAB>@evil.example/x.js` fetches from `evil.example`. The
        # `.split()` below exists for srcset's "url descriptor" pairs and splits on ALL
        # whitespace -- so with the tab still present it truncated the URL to exactly
        # `https://good.example`, which compares EQUAL to the origin. The pass did not merely
        # fail to report a cross-origin fetch, it affirmatively called it same-origin.
        #
        # Measured by security review; and then measured again here, because my first fix
        # stripped the tab AFTER the split, which changes nothing at all.
        cleaned = candidate.translate({0x09: None, 0x0A: None, 0x0D: None})
        for piece in cleaned.split(","):
            url = piece.strip().split()[0] if piece.strip() else ""
            if not url.lower().startswith(("http://", "https://")):
                continue
            # THE ORIGIN COMPARED STRUCTURALLY. This repository has had five defects where a
            # string operation stood in for a structural comparison -- `https://burrow.app`
            # accepting `https://burrow-app`, most recently in a sitemap rule three blocks
            # below the comment describing it. `urlsplit` splits on structure.
            parts = urllib.parse.urlsplit(url.rstrip(".,;)"))
            if f"{parts.scheme}://{parts.netloc}" != origin:
                found.append(url.rstrip(".,;)"))
    return sorted(set(found))



# How long to let the edge finish switching to the deployment we just uploaded, and how often
# to ask.
#
# WHY THIS EXISTS, MEASURED. `wrangler deploy` returns when the upload is accepted, not when
# the custom domain serves it, and this check runs in the step immediately after. On the deploy
# that brought PDFium back -- 5.3 MB and three new files -- the origin was still serving the
# PREVIOUS deployment about two seconds later, and this check reported sixteen files whose
# "live origin serves a body the build did not produce". The site was fine. The deployment was
# correct and complete; verified afterwards against the run's own artifact, all 43 files
# byte-identical, exit 0.
#
# A GATE THAT CRIES WOLF IS ONE PEOPLE LEARN TO RE-RUN UNTIL IT PASSES, which is exactly what
# this file's header says about gates whose verdict depends on an unstated precondition. The
# precondition here was "the edge has switched over", and it was neither stated nor waited for.
#
# WAITING DOES NOT WEAKEN IT, AND THE REASON IS THE SHAPE OF THE TWO FAILURES. A propagation
# lag converges: the origin is serving a DIFFERENT BUILD OF OURS and will stop. An edge feature
# that rewrites HTML never converges -- it rewrites whatever is served, so it is still wrong at
# the deadline. So the bound turns "fails if the edge is slow" into "fails if the edge is
# wrong", which is the question this file was always asking.
#
# The deadline is generous and the failure at the end of it is the full diff, unchanged.
#
# OVERRIDABLE, AND ONLY THE SELF-TEST MAY DO IT. Every "this must be refused" case in
# `tools/test-check-live-routes.sh` plants content that never converges -- that is what makes
# them cases -- so with a fixed deadline each one would wait out the full three minutes before
# failing. The suite went from seconds to unusable the moment this wait was added, which is how
# the knob came to exist.
#
# `tools/check-deploy-workflow.py` asserts no workflow sets it, because a deadline of zero in
# the deploy is the wait silently switched off -- and a check whose bound can be removed from
# outside is a check whose bound nobody can rely on.
DEADLINE_ENV = "BURROW_LIVE_PROPAGATION_DEADLINE_S"
PROPAGATION_DEADLINE_S = float(os.environ.get(DEADLINE_ENV, "180"))
PROPAGATION_POLL_S = 5.0


def await_propagation(origin: str, routes: list[tuple[str, pathlib.Path]]) -> str:
    """Wait until EVERY HTML route matches the build, and say what happened either way.

    EVERY ROUTE, NOT ONE WITNESS, AND THAT IS A CORRECTION MEASURED ON A REAL DEPLOY.

    This waited on `routes[0]` alone, on the argument that *"every route changes together --
    they come from one upload -- so one is as good an answer as forty-three"*. The deploy of
    #102 disproved it: the witness matched on the **first attempt**, the wait printed "the edge
    was already serving this deployment", and four other routes were still the previous
    deployment's. Cloudflare's edge does not switch every path at once.

    The shape of the mistake is worth keeping: the witness was chosen well (an HTML route,
    because HTML is what an edge feature rewrites, and a `.wasm` that matched would say nothing)
    and the sampling was the flaw. One sample of a set that does not move together is a probe
    that can report ready before anything is.

    **Only stable URLs can be stale at all.** Every engine artifact is content-hashed, so its URL
    is new on every build -- it either exists or 404s, and it can never serve a previous
    deployment's bytes. So the set that has to be waited on is exactly the HTML routes, which is
    seven requests a poll rather than forty-three.

    The full comparison then runs over everything regardless: this is a *wait*, never a
    substitute for the check.
    """
    built = {route: path.read_bytes() for route, path in routes}
    started = time.monotonic()
    deadline = started + PROPAGATION_DEADLINE_S
    attempts = 0
    stale: list[str] = [route for route, _ in routes]

    while True:
        attempts += 1
        still: list[str] = []
        for route in stale:
            try:
                if differences(built[route], fetch(f"{origin}{route}")):
                    still.append(route)
            except (Refused, urllib.error.URLError, OSError):
                # Not interpreted here. The full pass below fetches everything and reports a
                # fetch failure properly; swallowing it would be this function deciding
                # something. Treated as "not yet matched" so the wait does not end early on it.
                still.append(route)
        stale = still

        if not stale:
            # PRINTED EVERY RUN, INCLUDING WHEN IT WAITED FOR NOTHING. A wait nobody can see is
            # a wait nobody can tell is still there, and the deploy is the only place it ever
            # runs for real.
            waited = time.monotonic() - started
            if attempts > 1:
                print(
                    f"  waited {attempts} attempt(s) ({waited:.0f}s) for all {len(routes)} "
                    f"HTML route(s) to match the build; the edge was still serving the "
                    f"previous deployment"
                )
            else:
                print(
                    f"  no wait needed: all {len(routes)} HTML route(s) matched the build on "
                    f"the first attempt, so the edge was already serving this deployment"
                )
            return "matched"

        if time.monotonic() >= deadline:
            print(
                f"  {len(stale)} of {len(routes)} HTML route(s) still do not match the build "
                f"after {PROPAGATION_DEADLINE_S:.0f}s and {attempts} attempt(s): "
                f"{' '.join(stale)}. Comparing anyway, so the failure below is the diff rather "
                f"than a timeout."
            )
            return "timed-out"
        time.sleep(PROPAGATION_POLL_S)


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        fail("usage: tools/check-live-routes.py <origin> <dist>")
        return 2
    origin = argv[1].rstrip("/")
    dist = pathlib.Path(argv[2])

    if not origin.startswith(("https://", "http://")):
        fail(f"the origin must be a URL, got {origin}")
        return 2
    if not dist.is_dir():
        fail(f"no build at {dist}")
        return 2

    files = files_in(dist)
    if not files:
        fail(f"no files found under {dist}: the scan is wrong, not the build")
        return 2

    # EVERY FILE IN THE BUILD IS FETCHED, and the count says so against the build's own total.
    # A scan that found some of them would otherwise print OK over the rest.
    on_disk = sum(1 for path in dist.rglob("*") if path.is_file())
    excluded = sorted(
        name for name in PLATFORM_CONFIG if (dist / name).is_file()
    )
    if len(files) + len(excluded) != on_disk:
        fail(
            f"scanned {len(files)} file(s), excluded {len(excluded)}, and {dist} holds "
            f"{on_disk}; the scan is wrong"
        )
        return 2

    routes = [entry for entry in files if entry[1].name == "index.html"]

    # GATED ON A COUNT THE BUILD ITSELF STATES. Refusing over zero is the weak half: a glob
    # that found one route would print "1 route(s)" and OK, which is this repository's "4 of 15
    # reads as success". `sitemap.xml` is generated from the same build by
    # `tools/stage-web-engines.mjs` and lists exactly the landing routes, so it is an
    # INDEPENDENT count of the same fact -- and `check-deployable-build.sh` has already
    # required the two to agree before the upload. If Astro ever emitted `merge-pdf.html`
    # instead of `merge-pdf/index.html`, this scan would quietly find one route and this
    # comparison is what says so.
    sitemap = dist / "sitemap.xml"
    if sitemap.is_file():
        listed = len(re.findall(r"<loc>", sitemap.read_text(encoding="utf-8")))
        if listed != len(routes):
            fail(
                f"scanned {len(routes)} route(s) under {dist} and its sitemap lists {listed}. "
                f"One of the two is wrong, and until that is settled this check does not know "
                f"what it is supposed to be examining."
            )
            return 2
    else:
        fail(f"no sitemap.xml in {dist}, so the route count cannot be checked against anything")
        return 2

    print(
        f"check-live-routes: {len(files)} of {on_disk} file(s) in {dist} "
        f"({len(routes)} HTML route(s)), against {origin}"
    )
    print("  fetched as a BROWSER -- the rewrite that made this necessary is conditional on that")
    # PRINTED EVERY RUN, INCLUDING WHEN IT IS EMPTY. `if excluded:` meant the check said
    # nothing in the one case worth saying something about -- a build that had LOST its
    # `_headers`, which is a CSP regression, would produce silence here rather than a
    # statement. These are the only files this check does not compare, so they are the only
    # place it could be quietly examining nothing.
    print(
        "  not compared, consumed by the host as configuration: "
        + (" ".join(excluded) if excluded else "none")
    )
    if not (dist / "_headers").is_file():
        fail(
            f"{dist} has no root `_headers`. Every production build emits one -- it carries "
            f"the response-header policy -- so its absence is a regression rather than a "
            f"build that happens not to need it."
        )
        return 2

    # THE WAIT, BEFORE THE COMPARISON AND NOT INSTEAD OF IT. Over EVERY html route:
    # see `await_propagation` for the deploy that showed one witness is not enough.
    await_propagation(origin, routes)

    problems = 0
    checked = 0
    for route, path in files:
        url = f"{origin}{route}"
        try:
            served = fetch(url)
        except (Refused, urllib.error.URLError, OSError) as exc:
            fail(f"{route}: could not be fetched from the live origin ({exc})")
            problems += 1
            continue

        built = path.read_bytes()
        checked += 1

        # THE SECOND PASS IS FOR DOCUMENTS. It names what kind of difference an injection is,
        # and "kind" is a question about HTML. Every file gets the DIFF, which is the rule.
        foreign = foreign_references(served, origin) if path.name.endswith(".html") else []
        if foreign:
            fail(
                f"{route}: the served body references {len(foreign)} origin(s) this build does "
                f"not: {' '.join(foreign)}. No page may reference a third party "
                f"(CLAUDE.md, non-negotiable #1) -- and the claim has to be verifiable by "
                f"somebody watching the network tab, which a blocked request is not."
            )
            problems += 1

        diff = differences(built, served)
        if diff:
            fail(
                f"{route}: the live origin serves a body the build did not produce "
                f"({len(built)} bytes built, {len(served)} served). Something between `dist/` "
                f"and the browser edited the document."
            )
            for line in diff[:40]:
                print(f"    {line}")
            if len(diff) > 40:
                print(f"    … {len(diff) - 40} more diff line(s)")
            problems += 1

        if not foreign and not diff:
            print(f"  ok   {route}: byte-identical to the build, no foreign reference")

    if problems:
        fail(f"{problems} file check(s) failed")
        return 1
    if checked != len(files):
        fail(f"examined {checked} of {len(files)} file(s); the rest were never compared")
        return 1
    print(
        f"OK -- all {checked} live file(s) are byte-identical to the build "
        f"({len(routes)} HTML route(s) also scanned for foreign references)."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
