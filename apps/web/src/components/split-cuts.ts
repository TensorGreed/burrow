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

    if (!/^\d+$/.test(part)) {
      return {
        ok: false,
        problem: `"${part}" is not a page number. Give the pages to cut after, like 3, 7.`,
      };
    }
    const cut = Number(part);
    if (cut < 1) {
      return { ok: false, problem: "Pages are numbered from 1, so there is no page 0." };
    }
    if (cut >= pageCount) {
      // NOT `> pageCount`. A cut after the last page asks for a part with no pages in it,
      // which the core refuses -- so the page refuses it here, where the box is, rather than
      // after a round trip.
      return {
        ok: false,
        problem:
          pageCount === 1
            ? "This document has one page, so there is nowhere to cut it."
            : `A cut goes AFTER a page and there must be pages left over, so the last place to cut this document is after page ${pageCount - 1}.`,
      };
    }
    if (cut === previous) {
      return {
        ok: false,
        problem: `You have asked to cut after page ${cut} twice. Each cut makes one more part, so each place is named once.`,
      };
    }
    if (cut < previous) {
      // SORTED RATHER THAN REFUSED WOULD BE WRONG HERE, and this is the difference from
      // `page-selection.ts`. A selection is a set and tidying it is kind; a cut list that
      // arrived out of order means the person is thinking about it differently from the way
      // it will happen, and quietly reordering hides that.
      return {
        ok: false,
        problem: `Cuts are made in order, so ${cut} cannot come after ${previous}. List them smallest first.`,
      };
    }
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
