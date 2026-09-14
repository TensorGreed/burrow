// The shared delivery path, driven without a browser.
//
// `tool-delivery.ts` exists because the same security defect was found twice in three copies
// of this machinery -- one document's bytes offered under another document's name -- and the
// third copy has never been looked at. A shared fix with no test is that risk one layer along,
// so every ordering that produced a finding is enumerated here, plus the ones that did not.
//
// The orderings are what a person does, in the order they do them: choose, start, change their
// mind, choose again. Each test names which of the two findings it pins.

import { describe, expect, it } from "vitest";

import { createDelivery } from "./tool-delivery.js";
import type { ObjectUrls } from "./tool-delivery.js";

/** Object URLs that count themselves, so "created none" is a measurement. */
function urls(): ObjectUrls & { created: string[]; revoked: string[] } {
  const created: string[] = [];
  const revoked: string[] = [];
  return {
    created,
    revoked,
    create() {
      const url = `blob:fake/${created.length}`;
      created.push(url);
      return url;
    },
    revoke(url) {
      revoked.push(url);
    },
  };
}

const bytes = () => new Blob([new Uint8Array([1, 2, 3])]);

describe("createDelivery", () => {
  it("hands out the bytes under the captured name", () => {
    const u = urls();
    const delivery = createDelivery(u);
    const run = delivery.begin({ name: "report-rotated.pdf", signature: "90:1-3" });

    const handout = run.hand(bytes());

    expect(handout).toEqual({
      url: "blob:fake/0",
      name: "report-rotated.pdf",
      signature: "90:1-3",
    });
  });

  it("refuses a run invalidated while it was in flight, and creates no URL", () => {
    // ROTATE'S FINDING. Choose a private document, press the button, change your mind, choose
    // an innocuous one: the first operation's reply still arrives. It must not be handed out,
    // and -- the half that a `return` in the island would get wrong -- no URL may be created
    // for it, because an unrevoked object URL holds the bytes for the life of the page.
    const u = urls();
    const delivery = createDelivery(u);
    const run = delivery.begin({ name: "private-rotated.pdf" });

    delivery.invalidate();

    expect(run.live()).toBe(false);
    expect(run.hand(bytes())).toBeNull();
    expect(u.created).toEqual([]);
  });

  it("refuses the earlier of two runs and allows the later", () => {
    // The same thing by a different route: not a cancel, but a second operation started while
    // the first was still out. Both replies arrive; only one is current.
    const u = urls();
    const delivery = createDelivery(u);
    const first = delivery.begin({ name: "first.pdf" });
    const second = delivery.begin({ name: "second.pdf" });

    expect(first.live()).toBe(false);
    expect(first.hand(bytes())).toBeNull();
    expect(second.hand(bytes())).toEqual({
      url: "blob:fake/0",
      name: "second.pdf",
      signature: "",
    });
    expect(u.created).toHaveLength(1);
  });

  it("carries the signature captured before the await, not the one current after it", () => {
    // REORDER'S FINDING, and the one the shape of this API is aimed at. The island's
    // `request` is derived over the LIVE controls: assigning it when the reply lands records
    // what the box says THEN, so a person who edits the order mid-run gets a link the
    // staleness guard judges fresh, under a preview showing an order the bytes are not in.
    //
    // Here the signature is an argument to `begin`, so "after the await" is not a moment at
    // which it can be read at all. What this test can still check is that `hand` returns the
    // captured value rather than anything the caller passes it later -- and `hand` takes only
    // bytes, which is the assertion.
    const delivery = createDelivery(urls());
    let live = "4,3,2,1";
    const run = delivery.begin({ name: "doc-reordered.pdf", signature: live });

    live = "1,2,3,4";

    expect(run.hand(bytes())?.signature).toBe("4,3,2,1");
  });

  it("keeps the captured name even when the source it was derived from changes", () => {
    // The other half of rotate's finding: the name. A page that calls `suggestedName()` after
    // the await re-derives it from whatever file is chosen now.
    const delivery = createDelivery(urls());
    let chosen = "private";
    const run = delivery.begin({ name: `${chosen}-rotated.pdf` });

    chosen = "holiday";

    expect(run.hand(bytes())?.name).toBe("private-rotated.pdf");
  });

  it("revokes exactly the handout it is given, and tolerates null", () => {
    const u = urls();
    const delivery = createDelivery(u);
    const handout = delivery.begin({ name: "a.pdf" }).hand(bytes());

    delivery.release(null);
    expect(u.revoked).toEqual([]);

    delivery.release(handout);
    expect(u.revoked).toEqual(["blob:fake/0"]);
  });

  it("stays live across an await when nothing invalidated it", () => {
    // THE CONTROL. Every refusal above is satisfied by a helper that refuses everything; this
    // is what says the guard discriminates.
    const delivery = createDelivery(urls());
    const run = delivery.begin({ name: "a.pdf", signature: "s" });

    expect(run.live()).toBe(true);
    expect(run.hand(bytes())).not.toBeNull();
  });

  it("a watch does not invalidate a run in flight", () => {
    // A page count for a newly chosen file must not cancel an operation. `watch` reads the
    // generation; `begin` claims it. Getting this backwards would make choosing a file during
    // a merge silently discard the merge.
    const delivery = createDelivery(urls());
    const run = delivery.begin({ name: "a.pdf" });

    delivery.watch();

    expect(run.live()).toBe(true);
  });

  it("a watch is invalidated by anything that claims the generation after it", () => {
    const delivery = createDelivery(urls());
    const watch = delivery.watch();
    expect(watch.live()).toBe(true);

    delivery.invalidate();

    expect(watch.live()).toBe(false);
  });

  it("invalidating before any run has begun is harmless", () => {
    // `choose()` calls `invalidate()` unconditionally, including on the first file of a page.
    const delivery = createDelivery(urls());
    delivery.invalidate();

    expect(delivery.begin({ name: "a.pdf" }).live()).toBe(true);
  });
});
