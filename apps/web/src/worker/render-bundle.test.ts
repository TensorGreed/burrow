// The render bundle supplies the protocol what the protocol requires of it.
//
// `worker-protocol.js` is byte-identical in both bundles and reads three names from whichever
// file is concatenated after it — `init`, `KNOWN_OPS` and `runOperation`. That contract is
// prose plus hoisting, so a bundle that lost one of the three would fail in the browser, at the
// first message, as a `ReferenceError` the worker reports as a content-free `Internal`.
//
// The base bundle's half of this is covered by `init-memoisation.test.ts`, `input-budget.test.ts`
// and `multi-output.test.ts`, which all drive `MAIN`. `RENDER_MAIN` existed and was used by
// nothing — raised by code review, and the point is sharper than tidiness: a `test-scope.ts`
// export whose doc says "same order, same reason" reads as though the render bundle is
// exercised the same way, and it was not.

import { describe, expect, it } from "vitest";

import { RENDER_MAIN, load, operation } from "./test-scope.js";

describe("the render bundle's half of the protocol contract", () => {
  it("supplies init, KNOWN_OPS and runOperation", () => {
    // ALL THREE, BY NAME. A missing one is a `ReferenceError` inside the message handler in a
    // browser and nothing here; the protocol cannot assert it about itself, because it is the
    // half that does the reading.
    const declared = load(RENDER_MAIN).declared();
    for (const name of ["init", "KNOWN_OPS", "runOperation"]) {
      expect(
        declared[name],
        `render-main.js does not supply \`${name}\`, which worker-protocol.js reads`,
      ).toBeDefined();
    }
    expect(declared["ensureReady"], "the protocol's own half is missing").toBeDefined();

    // AND THE LIST IS THIS BUNDLE'S, not the other one's. Two bundles declaring the same three
    // names is the contract; declaring the same VALUES would mean the split did nothing.
    expect([...(declared["KNOWN_OPS"] as Set<string>)]).toEqual(["page_count", "render"]);
  });

  it("answers page_count and refuses every operation it does not have", async () => {
    // THE LIST IS THE BUNDLE'S, and the refusal is the protocol's. This is the one place the
    // two halves meet, so it is the one worth driving end to end without a browser.
    const harness = load(RENDER_MAIN);
    harness.releaseQpdf();
    await harness.ensureReady();

    await harness.send({ ...operation(1), op: "merge" });
    const refusal = harness.posted.find((m) => m["id"] === 1 && m["ok"] === false);
    expect(refusal, "an operation this bundle does not have was not refused").toBeDefined();
    expect(refusal?.["kind"]).toBe("InvalidArgument");
    expect(refusal?.["message"]).toBe("unknown operation");
    // NOT FATAL. An unknown op is a bug in the page, not a poisoned engine — three fatal
    // replies latch the circuit breaker, so getting this wrong takes rendering offline.
    expect(refusal?.["fatal"], "an unknown operation cost a worker").toBe(false);
  });

  it("refuses to touch a file when no policy is in force", async () => {
    // THE FAIL-CLOSED GUARD, in the second bundle. `prelude.js` is byte-identical in both, so
    // what this asserts is that the render bundle is still wired to it — not a second
    // implementation of it.
    const harness = load(RENDER_MAIN, false);
    await harness.send(operation(1));
    expect(harness.blobReads(), "it read a file with no policy in force").toBe(0);
    const refusal = harness.posted.find((m) => m["id"] === 1);
    expect(String(refusal?.["message"])).toContain("no policy in force");
  });
});
