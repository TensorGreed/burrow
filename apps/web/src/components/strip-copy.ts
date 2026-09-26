/**
 * The strip's withdrawal, and the sentences that have to agree with it.
 *
 * # Why this module exists rather than a boolean and four careful edits
 *
 * `STRIP_WITHDRAWN` decides whether `/split-pdf` and `/rotate-pdf` mount the page-picture
 * strip. Four pages describe that strip in prose: those two, `/reorder-pdf` (which says the
 * other two show pictures and it does not), and `/credits` (which over-declares the licence
 * components that arrive only with the second engine).
 *
 * **A boolean that silently makes four pages' copy false is a trap when it flips back.** The
 * person restoring the strip is exactly the person who will not remember which paragraphs to
 * rewrite -- the same reasoning the root `CLAUDE.md` gives for why a rule nobody can check is
 * not a control. So the pages BRANCH on the flag and take their opening words from the marks
 * below, and `strip-copy.test.ts` asserts, over the BUILT HTML of all four, that the mark for
 * the current state is present and the mark for the other state is absent.
 *
 * Restoring the strip is flipping one boolean. The copy follows it, and if a page ever stops
 * branching, the test says which page.
 */

/**
 * Whether the page-picture strip is withdrawn from the two pages that mount it.
 *
 * **`false` since 2026-09-26: mounted.** It was `true` from 2026-09-17, a withdrawal rather than
 * a removal -- the component, its tests, the render worker bundle and PDFium stayed exactly as
 * they were, and this decided only whether the islands mount it. It stays as the switch.
 *
 * # Why it was withdrawn
 *
 * On WebKit, `/split-pdf` and `/rotate-pdf` lost the tab -- or never delivered an operation's
 * output -- about **four times in 580 runs**, and **zero** with the strip absent. Losing a tab
 * mid-operation is the worst failure this site has, so the strip went the same day.
 *
 * # Two mechanisms were recorded, and both were wrong
 *
 * "A tab holding two engines", then "PDFium resident while an operation runs". Both were
 * inferred from failure rates across arms. The second was built into a fix -- the strip pausing
 * during an operation, the host refusing to acquire an engine, the resume deferred past the
 * paint -- and it halved the rate without removing it (**4 in 1,160**), which is what showed
 * the mechanism was not the one described. Those three pieces stay, for reasons of their own
 * (see `PageThumbnails`' `paused`); none of them is what fixed this. One of them was itself
 * unsound as built: the pause terminated the render worker mid-render, which crashes Firefox's
 * content process. It now waits for the in-flight render to settle; see the pause's effect.
 *
 * # What it actually was: a race in Linux WebKit, reached through a GPU canvas
 *
 * Collected directly on 2026-09-26, in [#107](https://github.com/TensorGreed/burrow/issues/107):
 * the WPE web process's main thread trapped in Skia's `SK_ABORT`, because Skia's compile of its
 * own premultiply round-trip shader "failed". `SkSL::stoi` tests `errno`, and WebKit's
 * thread-suspend signal handler leaves `errno` as EINTR. Three crashes in 580 runs, **zero in
 * 1,160** with only that handler patched to preserve `errno`. Not memory: WebKit logged
 * `reason=Crash`, and every process was at an ordinary footprint.
 *
 * The strip reached it because its thumbnails were `putImageData` into a GPU-backed canvas. It
 * now asks for a CPU canvas (`PageThumbnails`' `paint`), and the bar written on 2026-09-17,
 * before any fix existed -- the #107 loop clean over at least 1,160 runs on the arm that showed
 * it -- was met on 2026-09-26 with no patch to WebKit: **0 in 1,160**. Safari and iOS cannot hit
 * this at all; Apple's ports compile neither Skia nor the signal-based suspend.
 */
export const STRIP_WITHDRAWN = false;

/**
 * The words a page opens with when the strip is withdrawn.
 *
 * Each page continues in its own voice; what is shared is the phrase a reader recognises
 * across the site and the test can anchor on. They are here rather than typed into four
 * `.astro` files so that the assertion and the prose cannot drift apart.
 */
export const WITHDRAWN_MARK = "There are no page pictures here";

/** The words those pages open with when the strip is mounted. */
export const PRESENT_MARK = "Pages are shown as pictures";

/**
 * What `/credits` says about the second engine's components.
 *
 * The credits page over-declares deliberately -- ADR 0026: honest in the direction it is wrong
 * rather than silently wrong -- so it names components a visitor may not have received. With
 * the strip withdrawn, nothing on the site fetches that engine at all, and the paragraph has
 * to say so instead of describing a strip that is not there.
 */
export const CREDITS_WITHDRAWN_MARK = "Nothing here fetches that second engine at the moment";

/** What `/credits` says when the strip is mounted and the engine is reachable. */
export const CREDITS_PRESENT_MARK = "Some of it arrives only if you look at page pictures";

/**
 * `/reorder-pdf`'s own pair, because the shared one is ambiguous there.
 *
 * That page says "There are no page pictures here **yet**" when the strip is mounted elsewhere
 * -- reordering never had them, for a reason of its own -- so `WITHDRAWN_MARK` is a substring
 * of its MOUNTED copy, and a test anchored on the shared mark passed whatever that page said.
 * Found by flipping the flag and rebuilding, which is the one thing that exercises both states.
 */
export const REORDER_WITHDRAWN_MARK = "there are none on the other pages either";

/** What `/reorder-pdf` says when the other two pages do show pictures. */
export const REORDER_PRESENT_MARK = "reordering does not, and that is a decision";

/** Every route whose prose has to agree with `STRIP_WITHDRAWN`. The count is the measurement. */
export const PAGES_THAT_DESCRIBE_THE_STRIP = [
  "split-pdf/index.html",
  "rotate-pdf/index.html",
  "reorder-pdf/index.html",
  "credits/index.html",
] as const;
