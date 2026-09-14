// "3, 1, 9-5" as a new page order.
//
// A PURE FUNCTION, so every branch is tested without a browser — the same shape as
// `page-selection.ts`, and deliberately NOT the same function. A selection is a set: it is
// sorted and de-duplicated, because "1, 1" from a person means one page. An order is a
// sequence: `3, 1` and `1, 3` are different documents, and `1, 1` is not a mistake worth
// tidying away — it is a request that would lose a page, which is the failure this whole
// operation is careful about.
//
// # It completes a partial order, and that is a stated rule rather than a guess
//
// A permutation of a 500-page document is 500 numbers. Requiring all of them would make the
// tool unusable for the thing people actually want — "move this page to the front" — so a
// partial order is accepted and completed: **the pages you list come first, in the order you
// list them, and every page you did not list keeps its current order after them.**
//
// That is a rule, not an inference. The page states it in a sentence beside the box AND shows
// the resulting order, so the rule is observable rather than described. `apps/web/CLAUDE.md`'s
// design brief: the interface reports, it does not reassure.
//
// # Descending ranges are allowed here, and mean what they say
//
// `9-5` is pages 9, 8, 7, 6, 5 — which is how "reverse these" is expressed, and the whole
// Reverse preset is the box filled with `N-1`. `page-selection.ts` has no use for that (a set
// has no direction) and does not accept it.
//
// # And it bounds its own expansion
//
// Every endpoint is range-checked BEFORE the range is expanded, so `1-99999999999` is refused
// rather than allocating. The same ordering `burrow_ops::every` needed after `every(1, u64::MAX)`
// allocated until it panicked.

/** What a typed order turned out to be. */
export type Order =
  | {
      ok: true;
      /** The complete new order, one-based, every page exactly once. */
      order: number[];
      /** How many pages the person actually named, before completion. */
      named: number;
    }
  | {
      ok: false;
      /** What is wrong, said to a person: what they typed, and what would work. */
      problem: string;
    };

/** The largest number of characters an order may carry, so a paste cannot hang the tab. */
const MOST_CHARACTERS = 20_000;

/**
 * Resolve a typed order against a document of `pageCount` pages.
 *
 * An empty box is the document's current order — a no-op rather than an error, because an
 * empty box is not a mistake, it is somebody who has not typed anything yet.
 */
export function resolveOrder(input: string, pageCount: number): Order {
  if (pageCount <= 0) {
    return { ok: false, problem: "This document has no pages to put in order." };
  }
  if (input.length > MOST_CHARACTERS) {
    return {
      ok: false,
      problem: `That is longer than this box accepts. Give the pages you want moved — the rest keep their place.`,
    };
  }

  const named: number[] = [];
  const seen = new Set<number>();

  for (const rawPart of input.split(",")) {
    const part = rawPart.trim();
    if (part === "") continue;

    const range = part.match(/^(\d+)\s*-\s*(\d+)$/);
    if (range) {
      const from = Number(range[1]);
      const to = Number(range[2]);
      // CHECKED BEFORE EXPANDED. Both endpoints, against the document, so a huge range is a
      // refusal rather than an allocation.
      const bad = [from, to].find((n) => n < 1 || n > pageCount);
      if (bad !== undefined) {
        return { ok: false, problem: outOfRange(bad, pageCount) };
      }
      const step = from <= to ? 1 : -1;
      for (let n = from; step > 0 ? n <= to : n >= to; n += step) {
        if (seen.has(n)) return { ok: false, problem: twice(n) };
        seen.add(n);
        named.push(n);
      }
      continue;
    }

    if (!/^\d+$/.test(part)) {
      return {
        ok: false,
        problem: `"${part}" is not a page number. Use numbers and ranges, like 3, 1, 9-5.`,
      };
    }
    const page = Number(part);
    if (page < 1 || page > pageCount) {
      return { ok: false, problem: outOfRange(page, pageCount) };
    }
    if (seen.has(page)) return { ok: false, problem: twice(page) };
    seen.add(page);
    named.push(page);
  }

  // THE COMPLETION. Everything not named, in its current order, after everything named.
  const rest: number[] = [];
  for (let n = 1; n <= pageCount; n += 1) {
    if (!seen.has(n)) rest.push(n);
  }

  return { ok: true, order: [...named, ...rest], named: named.length };
}

/** Whether an order would leave the document exactly as it is. */
export function isUnchanged(order: number[]): boolean {
  return order.every((page, index) => page === index + 1);
}

function outOfRange(page: number, pageCount: number): string {
  if (page === 0) {
    return "Pages are numbered from 1, so there is no page 0.";
  }
  return `This document has ${pageCount} ${pageCount === 1 ? "page" : "pages"}, so there is no page ${page}.`;
}

function twice(page: number): string {
  // NOT DE-DUPLICATED, deliberately. In a selection "1, 1" means one page; in an ORDER it
  // means a document with page 1 twice and some other page missing, which is the loss this
  // operation exists to make impossible. Refused with the number, so it can be found.
  return `Page ${page} is listed twice. An order names each page once — every page comes through exactly once.`;
}
