// How a tool page hands a person the bytes it produced, in one place.
//
// # Why this is shared when the rest of the three islands deliberately is not
//
// The same argument `tool-host.ts` opens with, and one step further along. That file is the
// worker wiring; this is the thing that decides **whose bytes a person is handed, and under
// what name**. It has produced two security findings already, both of the same class -- one
// document's bytes offered under another document's name -- in two of the three copies:
//
//   * `RotateTool.svelte`: `choose()` did not invalidate an in-flight operation, so the reply
//     still counted as current and `suggestedName()` re-derived the name from the file chosen
//     *since*. The page showed a download link reading `holiday-rotated.pdf` whose bytes were
//     the private document opened a moment earlier. Nothing on screen distinguished it.
//   * `ReorderTool.svelte`: the request signature was read back *after* the awaits, so editing
//     the order mid-run produced a link the staleness guard judged fresh, sitting under a
//     preview showing an order the bytes were not in. The identical line sat in `rotate`.
//
// `MergeTool.svelte` is the third copy, and it calls `suggestedName()` after its awaits too.
// It escapes the finding only because it disables every control while working -- which is a
// property of its markup, not of its delivery, and is one `disabled` attribute away from not
// being true.
//
// # What the API does about it
//
// The discipline that prevents all three is "capture before the first await", and in three
// copies it lived in comments. Here it is the shape of the call:
//
// ```ts
// const run = delivery.begin({ name: suggestedName(), signature: request });  // before await
// const reply = await h.run(...);
// const handout = run.hand(reply.output);   // null if anything has moved since
// if (!handout) return;
// result = handout;                          // carries the CAPTURED name and signature
// ```
//
// `hand()` is the only way to get a URL, it refuses a stale run without creating one, and it
// has no access to anything live -- so there is nothing for a later edit to re-derive from.
// Re-deriving the name after the await is not "discouraged"; it is not expressible.

/** What a person is handed: a URL over the bytes, under the name captured for them. */
export interface Handout {
  /** An object URL over the produced bytes. The page never reads them into its own heap. */
  readonly url: string;
  /** The download name, captured before the operation was posted. */
  readonly name: string;
  /**
   * What was asked for, captured before the operation was posted.
   *
   * A page compares this against its live controls to decide whether what is on offer is
   * still the answer to what the box says. Empty where a tool has nothing to compare --
   * `merge`'s request is the file list itself, which it invalidates directly.
   */
  readonly signature: string;
}

/** One operation's claim on the page's output. */
export interface Run {
  /**
   * Whether this run is still the current one.
   *
   * **Call after every `await`.** A cancelled operation still answers: `discardWorker()`
   * terminates the worker and the host fails every in-flight request (ADR 0015), which is
   * correct from its point of view and a lie to a person who pressed Stop. Replies are
   * ignored by generation rather than by suppressing the failure branch, so a genuine failure
   * arriving a moment late is still reported.
   */
  live(): boolean;
  /**
   * Hand `bytes` out, under the name and signature captured when this run began.
   *
   * Returns `null` -- and creates no URL, so there is nothing to revoke -- if anything has
   * invalidated the run since. That check is inside rather than beside the call because
   * beside it is where it was forgotten twice.
   */
  hand(bytes: Blob): Handout | null;
}

/** The page's delivery path: generations, capture, and the URLs. */
export interface Delivery {
  /**
   * Invalidate anything in flight.
   *
   * Called when the document changes, when the person cancels, and before the worker is
   * discarded -- so the reply the discard provokes is already stale when it arrives rather
   * than racing the line that would have made it so.
   */
  invalidate(): void;
  /** Begin a run, capturing what its handout will carry. Bumps the generation. */
  begin(captured: { name: string; signature?: string }): Run;
  /**
   * Observe the current generation without claiming it, for work that hands nothing out.
   *
   * A page count is the case: it reads a document and writes a number beside a filename, and
   * a count still pending for the PREVIOUS file resolves after the page has already swapped
   * both -- writing the old document's page count beside the new document's name, which the
   * order box then validates against. It must not bump the generation, because counting a
   * file is not a reason to invalidate an operation in flight.
   */
  watch(): Pick<Run, "live">;
  /** Revoke a handout's URL, so the browser can release the bytes. Safe on `null`. */
  release(handout: Handout | null): void;
}

/**
 * The browser's object-URL pair, injectable so the unit tests can count the calls.
 *
 * Counting them is the point: "a stale run hands out nothing" is satisfied by a helper that
 * creates a URL and discards it, which leaks the bytes for the life of the page.
 */
export interface ObjectUrls {
  create(bytes: Blob): string;
  revoke(url: string): void;
}

const BROWSER_URLS: ObjectUrls = {
  create: (bytes) => URL.createObjectURL(bytes),
  revoke: (url) => URL.revokeObjectURL(url),
};

/** One delivery path per island. */
export function createDelivery(urls: ObjectUrls = BROWSER_URLS): Delivery {
  let generation = 0;

  return {
    invalidate() {
      generation += 1;
    },

    begin(captured) {
      // CAPTURED HERE, by value, and never read again from anywhere live. The `name` argument
      // is a string the caller has already computed; `hand()` cannot reach the file, the
      // entry list or the controls it came from.
      const name = captured.name;
      const signature = captured.signature ?? "";
      const mine = ++generation;

      const live = () => mine === generation;

      return {
        live,
        hand(bytes) {
          if (!live()) return null;
          return { url: urls.create(bytes), name, signature };
        },
      };
    },

    watch() {
      const mine = generation;
      return { live: () => mine === generation };
    },

    release(handout) {
      if (handout) urls.revoke(handout.url);
    },
  };
}
