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
 * **`true` since 2026-09-17, and this is a withdrawal rather than a removal.** The component,
 * its tests, the render worker bundle and the PDFium engine all stay exactly as they are; this
 * decides whether the islands mount it.
 *
 * # What was measured
 *
 * On WebKit, `/split-pdf` and `/rotate-pdf` -- the two pages that mount this strip -- lose the
 * tab, or fail to deliver an operation's output, about **four times in 580 runs**. With the
 * strip suppressed on the same commit, and on the commit before it existed, **zero in 580**.
 * No page without the strip has ever failed this way.
 *
 * **THE MECHANISM IS PDFIUM RESIDENT WHILE AN OPERATION RUNS, NOT "TWO ENGINES AT ONCE".** The
 * first framing was wrong and is corrected here rather than quietly dropped: the DOCUMENTS
 * worker persists after an operation, so qpdf and PDFium are both resident whenever the strip
 * draws at all. What distinguishes the failing case is an operation running while PDFium is
 * still there -- 1.9 MB with a 2 GiB heap ceiling, next to a qpdf heap doing the work.
 * [ADR 0015](../../../../docs/adr/0015-web-worker-lifecycle.md) §7 recorded that shape on iOS;
 * this is it on desktop WebKit.
 *
 * **Losing a tab in the middle of an operation is the worst failure this site has** -- worse
 * than not showing thumbnails, which is how both pages worked for their whole life until the
 * morning of the day this flag was set. So the strip goes, today, on one line that is measured
 * to remove the failure, rather than the exposure standing while the real fix is designed.
 *
 * # What brings it back
 *
 * Not a hunch and not a quiet flip: PDFium released for the duration of an operation, and then
 * the loop in [#107](https://github.com/TensorGreed/burrow/issues/107) run clean over **at
 * least 1,160 runs**, twice the sample that exposed the defect, on the arm that currently shows
 * it. The bar is written down here, before the fix is built, for the same reason ADR 0027's
 * progressive-render bar was: a bar set after the numbers is not a bar.
 *
 * **AND THE BAR MEASURES THE OUTCOME, NOT THE PROPERTY.** A clean 1,160 says the failure did
 * not occur; it does not say an engine is never acquired during an operation. No test proves
 * that end to end -- see [#114](https://github.com/TensorGreed/burrow/issues/114), which
 * records why the one written for it could not be made to fail and what an adequate one needs.
 *
 * # The bar was run on 2026-09-18 and NOT met
 *
 * The fix was built -- the strip paused for the duration of an operation, acquisition refused
 * at the host, the resume deferred past the paint -- and measured against the bar above:
 *
 * | arm | failures / runs |
 * |---|---|
 * | strip mounted, before any of it | 4 / 580 |
 * | strip mounted, WITH the fix | **4 / 1,160** |
 * | strip absent | 0 / 1,160 |
 *
 * **The rate roughly halved. The failure did not go away.** Four crashes, all
 * `Target page, context or browser has been closed`, on both pages. A twofold improvement in a
 * crash rate is not a fix, and the bar is what caught that -- an earlier reading of "roughly
 * eightfold" came from one clean pair of runs and was wrong.
 *
 * **It also undermines the mechanism this file describes.** If PDFium's absence during an
 * operation were the whole story, gating acquisition three ways should have removed the crash.
 * It did not, so something else is involved and nothing here names it. The description above is
 * what the arms support; it is not an explanation of the remainder.
 *
 * WHAT THE NEXT ATTEMPT SHOULD DO IS NOT ANOTHER GATE. Every conclusion so far -- including the
 * two that were wrong -- was inferred from correlation across arms, and the direct evidence has
 * never been collected: no WebKit crash log has been captured, only Playwright reporting that
 * the page died. That is the gap to close first.
 */
export const STRIP_WITHDRAWN = true;

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
