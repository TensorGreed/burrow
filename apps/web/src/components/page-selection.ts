// "1, 4, 7-9" as a list of page numbers.
//
// A PURE FUNCTION, so every branch is tested without a browser. It is the only thing on the
// rotate page that turns what a person typed into something the core is asked to do, and the
// failure modes are all quiet ones: a selection that silently drops a page, or silently adds
// one, produces a document nobody can tell is wrong by looking at the operation that made it.
//
// # It parses a selection; it does not enforce a ceiling
//
// `apps/web/CLAUDE.md`: "The page does not re-implement a ceiling. It sends the files and
// reports what the core refuses." That rule is about `Limits` — `max_pages`, `max_input_bytes`
// — and it still holds here: nothing in this file knows what those are.
//
// What this DOES know is how many pages the document in front of the person has, which the
// tool has already asked the core for. Refusing "12" on a four-page document is answering a
// question about that document, not applying a policy, and the alternative is worse: the core
// would refuse it with "page is not in the document" after a round trip, which is the same
// answer arriving later and with less to say.
//
// # And it bounds its own expansion
//
// `1-99999999999` must not allocate eleven billion entries before anything checks it. Every
// endpoint is range-checked BEFORE the range is expanded — the same ordering `burrow_ops::every`
// needed after `every(1, u64::MAX)` allocated until it panicked with a capacity overflow. Here
// the ceiling is the document's own page count, which is small and known.

/** What a selection turned out to be. */
export type Selection =
  | { ok: true; pages: number[] }
  | {
      ok: false;
      /**
       * What is wrong, said to a person.
       *
       * Errors do not apologise and are never vague (the design brief's writing rules). Each
       * says what is wrong with what they typed and what a usable answer looks like.
       */
      problem: string;
    };

/** The largest number of characters a selection may carry, so a paste cannot hang the tab. */
const MOST_CHARACTERS = 2_000;

/**
 * Parse a page selection against a document of `pageCount` pages.
 *
 * Accepts a comma-separated list of single pages and `from-to` ranges, in any order, with any
 * spacing. Returns the pages **sorted and de-duplicated**, because the core refuses a page
 * named twice and "1, 1" from a person means one page rather than a mistake worth refusing.
 *
 * An empty selection is not an error here and not a success: it is
 * `{ ok: false }` with a problem saying so, because the tool's other control already means
 * "every page" and a blank box should not quietly mean the same thing.
 */
export function parseSelection(input: string, pageCount: number): Selection {
  if (input.length > MOST_CHARACTERS) {
    return {
      ok: false,
      problem: `That is a lot of text for a page list. Use ranges like 1-${pageCount} instead.`,
    };
  }

  const trimmed = input.trim();
  if (trimmed === "") {
    return { ok: false, problem: "Type which pages to turn, like 1-3 or 2, 5, 9." };
  }
  if (pageCount <= 0) {
    return { ok: false, problem: "This document has no pages to turn." };
  }

  const pages = new Set<number>();
  for (const rawPart of trimmed.split(",")) {
    const part = rawPart.trim();
    if (part === "") {
      // A trailing or doubled comma. Refused rather than skipped: "1,,3" is a typo, and
      // guessing which of the two readings was meant is how a selection silently changes.
      return {
        ok: false,
        problem: "There is an empty item in that list — check the commas.",
      };
    }

    // `1-3`, but not `-3`, `1-`, or `1-2-3`. Split rather than a regex over the whole string
    // so the error can name which part was wrong.
    const bounds = part.split("-");
    if (bounds.length > 2) {
      return { ok: false, problem: `"${part}" is not a page or a range. Try 1-3.` };
    }

    const numbers = bounds.map(parsePageNumber);
    if (numbers.some((n) => n === null)) {
      return {
        ok: false,
        problem: `"${part}" is not a page number. Use whole numbers, like 4 or 2-6.`,
      };
    }
    const [first, last] = [numbers[0] as number, (numbers[1] ?? numbers[0]) as number];

    // RANGE-CHECKED BEFORE EXPANDED. Both ends, before the loop below runs even once.
    for (const bound of [first, last]) {
      if (bound < 1) {
        return { ok: false, problem: "Pages are numbered from 1." };
      }
      if (bound > pageCount) {
        return {
          ok: false,
          problem: `This document has ${pageCount} page${pageCount === 1 ? "" : "s"}, so ${bound} is past the end.`,
        };
      }
    }
    if (first > last) {
      // `5-2`. Refused rather than swapped, for the same reason `1,,3` is: it is a typo, and
      // a tool that quietly reinterprets what you typed is one you cannot check.
      return {
        ok: false,
        problem: `"${part}" runs backwards. Put the lower page first, like ${last}-${first}.`,
      };
    }

    for (let page = first; page <= last; page += 1) {
      pages.add(page);
    }
  }

  return { ok: true, pages: [...pages].sort((a, b) => a - b) };
}

/**
 * One page number, or `null`.
 *
 * Deliberately strict: `Number("4 ")` is 4 and `Number("")` is 0, both of which would let
 * something through that a person did not type. Only digits, and at most nine of them — enough
 * for any document and not enough to reach `Number.MAX_SAFE_INTEGER`, so the comparisons above
 * are exact rather than approximate.
 */
function parsePageNumber(text: string): number | null {
  const part = text.trim();
  if (!/^\d{1,9}$/.test(part)) return null;
  return Number(part);
}
