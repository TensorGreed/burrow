// "3, 7" as the places a document is cut, and the parts that fall out of it.
//
// A PURE FUNCTION, so every branch is tested without a browser — the same shape as
// `reorder-order.ts` and `page-selection.ts`, and deliberately not either of them.
//
// # A cut is a gap, not a page
//
// `Cuts::after_pages` in `core/burrow-ops/src/split/mod.rs` takes one-based page numbers
// **after which a new document starts**, so `[3, 7]` on ten pages means 1-3, 4-7, 8-10. That
// off-by-one is the thing a person gets wrong, so the page shows the resulting parts rather
// than describing the rule — the same answer `/reorder-pdf` gave for its completion rule.
//
// A cut at the LAST page is refused rather than ignored. `10` on ten pages would ask for an
// eleventh part with no pages in it, and the core refuses it; saying so beside the box is
// better than a round trip that comes back `InvalidArgument`.
//
// # Why the page may compute the parts at all
//
// The core computes the same partition internally, and two implementations of one rule is
// what this repository has a rule against. This is the exception and the reason is narrow:
// the partition is **determined** by the cuts, which the core has already validated — it is
// arithmetic over numbers, not a heuristic, so the two cannot disagree about a cut list they
// both accept. What the page needs it for is the preview and the download names, and the
// name-against-bytes check is not left to that argument: `e2e/split-pdf.spec.ts` reads the
// page span out of each downloaded file's OWN name and asserts the part has that many pages.
// If these ever disagreed with the core, that test is what would say so.
//
// # And it bounds its own expansion
//
// Every cut is range-checked BEFORE the parts are built, so a huge number is a refusal rather
// than an allocation — the same ordering `burrow_ops::every` needed after `every(1, u64::MAX)`
// allocated until it panicked.

/** One output document: where it starts, and how many pages it has. */
export interface Part {
  /** One-based page number of this part's first page in the source. */
  readonly first: number;
  /** How many pages this part contains. Always at least 1. */
  readonly count: number;
}

/** What a typed cut list turned out to be. */
export type Cuts =
  | {
      ok: true;
      /** The cut points, one-based, strictly increasing. Empty means "do not cut". */
      cuts: number[];
      /** The documents those cuts produce, in order. `cuts.length + 1` of them. */
      parts: Part[];
    }
  | {
      ok: false;
      /** What is wrong, said to a person: what they typed, and what would work. */
      problem: string;
    };

/** The largest number of characters a cut list may carry, so a paste cannot hang the tab. */
const MOST_CHARACTERS = 20_000;

/**
 * Resolve a typed cut list against a document of `pageCount` pages.
 *
 * An empty box is not an error — it is somebody who has not typed anything yet. It resolves
 * to a single part covering the whole document, which is what "split this into one part"
 * means and is the identity case the core's property tests use. The PAGE refuses to run it,
 * for the same reason `/reorder-pdf` refuses an identity: it would rewrite somebody's file to
 * no effect.
 */
export function resolveCuts(input: string, pageCount: number): Cuts {
  if (pageCount <= 0) {
    return { ok: false, problem: "This document has no pages to split." };
  }
  if (input.length > MOST_CHARACTERS) {
    return {
      ok: false,
      problem: "That is longer than this box accepts. Give the pages you want to cut after.",
    };
  }

  const cuts: number[] = [];
  let previous = 0;

  for (const rawPart of input.split(",")) {
    const part = rawPart.trim();
    if (part === "") continue;

    // A RANGE MEANS "CUT AFTER EACH OF THESE", and it is here because without it one of the
    // two things people most want from a splitter is unreachable. Splitting a 40-page
    // document into single pages needs 39 cut points typed by hand — 145 characters; a
    // 500-page one needs 2,385. `/reorder-pdf` reverses a 500-page document in five
    // characters because it has a range and a preset, and this page had neither.
    //
    // IT IS NOT `burrow_ops::every`. The person still says WHERE the cuts go; this expands a
    // range in a list the page already parses and the core still validates. "Every 10 pages"
    // is a different thing — a rule about spacing rather than a list of places — and it stays
    // in the core, unexposed, rather than being reimplemented here.
    const range = part.match(/^(\d+)\s*-\s*(\d+)$/);
    if (range) {
      const from = Number(range[1]);
      const to = Number(range[2]);
      // BOTH ENDPOINTS CHECKED BEFORE THE RANGE IS EXPANDED, so `1-99999999999` is a refusal
      // rather than an allocation — the same ordering `burrow_ops::every` needed after
      // `every(1, u64::MAX)` allocated until it panicked.
      for (const endpoint of [from, to]) {
        const refusal = outOfRange(endpoint, pageCount);
        if (refusal) return { ok: false, problem: refusal };
      }
      if (to < from) {
        // DESCENDING IS REFUSED, not reversed. `reorder-order.ts` reads `9-5` as a reversal
        // because an order has a direction; cuts are made in order and a descending range
        // describes nothing a split could do.
        return {
          ok: false,
          problem: `Cuts are made in order, so ${from}-${to} runs backwards. Give the range the other way round.`,
        };
      }
      for (let cut = from; cut <= to; cut += 1) {
        if (cut === previous) {
          return { ok: false, problem: twice(cut) };
        }
        if (cut < previous) {
          return { ok: false, problem: outOfOrder(cut, previous) };
        }
        cuts.push(cut);
        previous = cut;
      }
      continue;
    }

    if (!/^\d+$/.test(part)) {
      return {
        ok: false,
        problem: `"${part}" is not a page number. Give the pages to cut after, like 3, 7, or a range like 1-9.`,
      };
    }
    const cut = Number(part);
    const refusal = outOfRange(cut, pageCount);
    if (refusal) return { ok: false, problem: refusal };
    if (cut === previous) return { ok: false, problem: twice(cut) };
    if (cut < previous) return { ok: false, problem: outOfOrder(cut, previous) };
    cuts.push(cut);
    previous = cut;
  }

  return { ok: true, cuts, parts: partsFrom(cuts, pageCount) };
}

/**
 * The documents a validated cut list produces.
 *
 * `cuts` must already be validated — strictly increasing and strictly inside the document —
 * which is what `resolveCuts` guarantees.
 *
 * EXPORTED FOR ITS OWN TEST, and that is the whole reason. The island reads `resolved.parts`
 * and never calls this directly; the earlier comment said "for the preview and for naming",
 * which described a caller that does not exist. An overclaiming comment is a bug here rather
 * than a wording preference. Code review.
 */
export function partsFrom(cuts: readonly number[], pageCount: number): Part[] {
  const parts: Part[] = [];
  let start = 1;
  for (const cut of cuts) {
    parts.push({ first: start, count: cut - start + 1 });
    start = cut + 1;
  }
  parts.push({ first: start, count: pageCount - start + 1 });
  return parts;
}

/** Whether a cut list would leave the document as one whole document. */
export function isWholeDocument(parts: readonly Part[]): boolean {
  return parts.length <= 1;
}

/** Why `cut` is not a place this document can be cut, or `null` if it is. */
function outOfRange(cut: number, pageCount: number): string | null {
  if (cut < 1) return "Pages are numbered from 1, so there is no page 0.";
  if (cut < pageCount) return null;
  // NOT `> pageCount`. A cut after the last page asks for a part with no pages in it, which
  // the core refuses -- so the page refuses it here, where the box is, rather than after a
  // round trip.
  return pageCount === 1
    ? "This document has one page, so there is nowhere to cut it."
    : `A cut goes AFTER a page and there must be pages left over, so the last place to cut this document is after page ${pageCount - 1}.`;
}

function twice(cut: number): string {
  return `You have asked to cut after page ${cut} twice. Each cut makes one more part, so each place is named once.`;
}

function outOfOrder(cut: number, previous: number): string {
  // SORTED RATHER THAN REFUSED WOULD BE WRONG HERE, and this is the difference from
  // `page-selection.ts`. A selection is a set and tidying it is kind; a cut list that arrived
  // out of order means the person is thinking about it differently from the way it will
  // happen, and quietly reordering hides that.
  return `Cuts are made in order, so ${cut} cannot come after ${previous}. List them smallest first.`;
}

/**
 * The cut list that gives every page its own document.
 *
 * The page offers this as a preset that types into the box, the way /reorder-pdf's "Reverse
 * the order" does -- so what it did is visible and editable rather than a hidden mode.
 */
export function everyPageCuts(pageCount: number): string {
  return pageCount >= 3 ? `1-${pageCount - 1}` : "1";
}
