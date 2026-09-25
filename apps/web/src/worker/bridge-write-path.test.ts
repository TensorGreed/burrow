// The bytes a redaction writes are the bytes qpdf receives.
//
// `__burrow_qpdf_oh_replace_stream_data` is the only call in the bridge that carries bytes INTO
// a live object graph, and it is redaction's write. Every other bridge function hands qpdf a
// number or a handle, or copies bytes OUT; this one copies a rewritten content stream in, and
// what it copies is the whole product. A bridge that writes the right LENGTH of wrong bytes
// produces a valid PDF whose page draws something nobody wrote — and, for redaction, one where
// the operation reports a removal it did not make.
//
// # Why this is executed rather than read
//
// `bundle-files.test.ts` and `reply-shape.test.ts` read the committed source and assert on its
// text, which is right for the questions they ask. This one is about what the code DOES with a
// heap: the offsets, the copy, and the free. So the two bridge files are evaluated against a
// fake Emscripten module with a real `ArrayBuffer` behind it, and the assertion is on the bytes
// that landed.
//
// The bridge files are concatenated into `burrow-worker.js` rather than imported — they assign
// to `self.*` and share `bridge-common.js`'s helpers through the bundle scope — so they are
// loaded here the way the bundler joins them, in the order `stage-web-engines.mjs` lists.
//
// # The mutation this is built to fail on
//
// `writes the right length of wrong bytes` plants exactly that defect in a COPY of the bridge
// and requires the round-trip assertion to refuse it. A length check would pass the mutant, and
// so would a fake that recorded only how many bytes it was given — which is why `FakeQpdf` on
// the Rust side records the bytes in full too.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";

const workerDir = import.meta.dirname;

/** What the fake engine saw, in the order it saw it. */
type Seen = {
  data: number;
  stream: number;
  bytes: Uint8Array;
  filter: number;
  decodeParms: number;
};

type Fake = {
  module: Record<string, unknown>;
  seen: Seen[];
  live: () => number;
  /** Detach the heap for real, the way growing it does. */
  detach: () => void;
};

/**
 * An Emscripten module with a real heap, a bump allocator, and a free that is checked.
 *
 * The allocator does not reuse, so a pointer freed twice or used after a free is visible rather
 * than absorbed — the shape the `free(0xc0ffee)` defect had.
 */
function fakeQpdf(heapBytes = 1 << 16): Fake {
  let buffer = new ArrayBuffer(heapBytes);
  const seen: Seen[] = [];
  /** Outstanding allocations, pointer -> size, so `_free` can poison the region. */
  const outstanding = new Map<number, number>();
  // Never 0 for a non-zero request: the bridge treats 0 as an allocation failure, so an
  // allocator handing it out would make the refusal path fire on a perfectly good call.
  let next = 8;

  const module: Record<string, unknown> = {
    HEAPU8: new Uint8Array(buffer),
    HEAPU32: new Uint32Array(buffer),
    _malloc: (n: number) => {
      // `malloc(0)` MAY RETURN NULL, and here it always does. C leaves it implementation
      // defined; the bridge's guard exists for the implementations that return 0, so a fake
      // that never does cannot test the guard. Security review measured that: with
      // `Math.max(n, 1)` here, deleting the guard left every test green.
      if (n === 0) {
        return 0;
      }
      const at = next;
      next += n + 8;
      if (next > heapBytes) {
        return 0;
      }
      outstanding.set(at, n);
      return at;
    },
    _free: (ptr: number) => {
      const size = outstanding.get(ptr);
      if (size === undefined) {
        throw new Error(`free of a pointer that is not outstanding: ${ptr}`);
      }
      outstanding.delete(ptr);
      // POISONED, so a USE-AFTER-FREE is visible. Without this the bump allocator never
      // reuses and the freed bytes sit there intact, so moving the free above the engine call
      // — the textbook defect for this function — passed every test. Measured by security
      // review, which is the second time this file's own claim ("a pointer used after a free
      // is visible rather than absorbed") had to be made true rather than asserted.
      (module.HEAPU8 as Uint8Array).fill(0xde, ptr, ptr + size);
    },
    _qpdf_oh_replace_stream_data: (
      data: number,
      stream: number,
      buf: number,
      len: number,
      filter: number,
      decodeParms: number,
    ) => {
      // COPIED HERE, as qpdf copies: `qpdf-c.h` says the library copies the data and that it
      // need not remain valid after the call. Slicing models that, and it is also what makes
      // the assertion meaningful — a fake holding the pointer would read the bytes back after
      // the bridge freed them and could not tell a copy from a dangling read.
      seen.push({
        data,
        stream,
        bytes: (module.HEAPU8 as Uint8Array).slice(buf, buf + len),
        filter,
        decodeParms,
      });
    },
    _qpdf_oh_new_null: () => 77,
  };
  return {
    module,
    seen,
    live: () => outstanding.size,
    detach: () => {
      // REAL DETACHMENT, not a swap for a fresh view. `ArrayBuffer.prototype.transfer`
      // detaches the original, so every existing view over it throws on access — which is
      // what `-sALLOW_MEMORY_GROWTH=1` does to `HEAPU8` when the heap grows. Swapping in a
      // new view left the old buffer live, so a hoisted view wrote into stale memory and
      // threw nothing; the test caught the hoist by content while claiming to catch it by a
      // throw.
      const grown = buffer.transfer(heapBytes * 2);
      buffer = grown;
      module.HEAPU8 = new Uint8Array(grown);
      module.HEAPU32 = new Uint32Array(grown);
    },
  };
}

/** Evaluate the bridge the way the bundler concatenates it, and hand back its entry points.
 *
 * `runInNewContext` with `self === globalThis === scope`, as `test-scope.ts` does, rather than
 * `new Function("self", …)`. The generated wasm-bindgen glue calls these as BARE IDENTIFIERS
 * (`__burrow_qpdf_copy_in(...)`, resolved off the worker global), and a `self` that is a plain
 * parameter object does not model that — a bridge that forgot its `self.` prefix would pass.
 *
 * The file list is read from `stage-web-engines.mjs` rather than hardcoded, so a rename or a
 * reorder there is caught here instead of silently testing the wrong pair.
 */
function loadBridge(qpdfSource?: string): {
  scope: Record<string, (...args: never[]) => unknown>;
  attach: (module: unknown) => void;
} {
  const sources = bundleOrder().map((name) =>
    name === "bridge-qpdf.js" && qpdfSource !== undefined
      ? qpdfSource
      : readFileSync(join(workerDir, name), "utf8"),
  );
  const scope: Record<string, unknown> = {};
  scope.self = scope;
  scope.globalThis = scope;
  // `__burrow_attach` is a bundle-scope function rather than a `self.` global, so it is picked
  // up by evaluating an expression after the sources rather than read off the scope.
  const attach = runInNewContext(`${sources.join("\n")}\n;(__burrow_attach)`, scope) as (
    module: unknown,
  ) => void;
  return { scope: scope as Record<string, (...args: never[]) => unknown>, attach };
}

/** The two bridge files this test needs, in the order the base bundle concatenates them. */
function bundleOrder(): string[] {
  const stager = readFileSync(
    join(workerDir, "..", "..", "..", "..", "tools", "stage-web-engines.mjs"),
    "utf8",
  );
  const base = /id:\s*"worker",[\s\S]*?order:\s*\[([\s\S]*?)\n {4}\],/.exec(stager);
  if (!base) {
    throw new Error("could not read the base worker bundle's order from stage-web-engines.mjs");
  }
  const names = [...base[1].matchAll(/\bapp:\s*"src\/worker\/([\w.-]+)"/g)].map((m) => m[1]);
  const wanted = names.filter((n) => n === "bridge-common.js" || n === "bridge-qpdf.js");
  if (wanted.length !== 2) {
    throw new Error(`expected both bridge files in the bundle order, found ${wanted.join(", ")}`);
  }
  return wanted;
}

function replaceStreamData(
  scope: Record<string, (...args: never[]) => unknown>,
  ...args: unknown[]
): boolean {
  const fn = scope.__burrow_qpdf_oh_replace_stream_data;
  return fn(...(args as never[])) as boolean;
}

describe("the redaction write path", () => {
  it("hands qpdf exactly the bytes it was given", () => {
    const fake = fakeQpdf();
    const { scope, attach } = loadBridge();
    attach(fake.module);

    // Not a round number and not all-printable: a copy that got the length right and the
    // offset wrong, or that stopped at a NUL, has to be visible.
    const content = new Uint8Array([
      0x42, 0x54, 0x20, 0x00, 0xff, 0x28, 0x29, 0x5c, 0x0a, 0x45, 0x54, 0x80, 0x7f, 0x01,
    ]);
    const ok = replaceStreamData(scope, 1, 2, content, 3, 4);

    expect(ok).toBe(true);
    expect(fake.seen).toHaveLength(1);
    expect(fake.seen[0].bytes).toEqual(content);
    expect(fake.seen[0]).toMatchObject({ data: 1, stream: 2, filter: 3, decodeParms: 4 });
  });

  it("frees the buffer it allocated, and frees it once", () => {
    const fake = fakeQpdf();
    const { scope, attach } = loadBridge();
    attach(fake.module);

    for (let i = 0; i < 5; i += 1) {
      replaceStreamData(scope, 1, 2, new Uint8Array([i, i, i]), 0, 0);
    }
    // The fake's `_free` throws on a pointer it did not issue or has already taken back, so
    // reaching here at all rules out a double free; this rules out a leak.
    expect(fake.live()).toBe(0);
  });

  it("writes an empty stream without reporting a refusal", () => {
    // `_malloc(0)` may legitimately return 0, which is this function's signal for failure. An
    // emptied stream is exactly what a redaction that removed everything produces.
    const fake = fakeQpdf();
    const { scope, attach } = loadBridge();
    attach(fake.module);

    const ok = replaceStreamData(scope, 1, 2, new Uint8Array(0), 0, 0);

    expect(ok).toBe(true);
    expect(fake.seen[0].bytes).toEqual(new Uint8Array(0));
    expect(fake.live()).toBe(0);
  });

  it("refuses rather than throws when the engine heap cannot take the bytes", () => {
    const fake = fakeQpdf(64);
    const { scope, attach } = loadBridge();
    attach(fake.module);

    const ok = replaceStreamData(scope, 1, 2, new Uint8Array(4096), 0, 0);

    expect(ok).toBe(false);
    expect(fake.seen).toHaveLength(0);
  });

  it("reads the heap view after the allocation, so a grown heap cannot detach it", () => {
    // `-sALLOW_MEMORY_GROWTH=1` DETACHES every view over the old buffer, and `_malloc` is
    // where growth happens. `fake.detach()` does it for real with `ArrayBuffer.transfer`, so
    // a bridge that hoisted `module.HEAPU8` above the malloc writes into a detached view and
    // THROWS — which is what this test's comment used to claim while modelling a swap that
    // could not throw.
    const fake = fakeQpdf();
    const original = fake.module._malloc as (n: number) => number;
    fake.module._malloc = (n: number) => {
      const at = original(n);
      fake.detach();
      return at;
    };

    const { scope, attach } = loadBridge();
    attach(fake.module);
    const content = new Uint8Array([9, 8, 7, 6]);

    expect(replaceStreamData(scope, 1, 2, content, 0, 0)).toBe(true);
    expect(fake.seen[0].bytes).toEqual(content);
  });

  it("does not free the buffer before the engine has read it", () => {
    // THE USE-AFTER-FREE, which this file's fake could not see until `_free` poisoned the
    // region it takes back. `qpdf copies before returning` is the single load-bearing
    // assumption behind freeing in a `finally`, and it is the assumption a qpdf version bump
    // could invalidate — so the harness has to be able to catch the free moving.
    const source = readFileSync(join(workerDir, "bridge-qpdf.js"), "utf8");
    const original = "module._qpdf_oh_replace_stream_data(";
    expect(source).toContain(original);
    const mutated = source.replace(
      original,
      "module._free(buf), module._qpdf_oh_replace_stream_data(",
    );
    expect(mutated).not.toEqual(source);

    const fake = fakeQpdf();
    const { scope, attach } = loadBridge(mutated);
    attach(fake.module);

    const content = new Uint8Array([1, 2, 3, 4]);
    // The double free in the `finally` is itself caught by the fake, so either the poisoned
    // bytes or the throw refuses this — both are the defect, and neither is silence.
    let refused = false;
    try {
      replaceStreamData(scope, 1, 2, content, 0, 0);
      refused = !fake.seen[0] || !Buffer.from(fake.seen[0].bytes).equals(Buffer.from(content));
    } catch {
      refused = true;
    }
    expect(refused).toBe(true);
  });

  it("writes the right length of wrong bytes — and the round trip refuses it", () => {
    // THE MUTATION, and the reason the assertion above compares bytes rather than a length.
    const source = readFileSync(join(workerDir, "bridge-qpdf.js"), "utf8");
    const original = "module.HEAPU8.set(bytes, buf);";
    // ASSERT THE MUTATION APPLIED before running anything. A `replace` that matched nothing
    // leaves a green test that reads as "this defence works" while nothing was mutated.
    expect(source).toContain(original);
    const mutated = source.replace(original, "module.HEAPU8.fill(0x41, buf, buf + bytes.length);");
    expect(mutated).not.toEqual(source);

    const fake = fakeQpdf();
    const { scope, attach } = loadBridge(mutated);
    attach(fake.module);

    const content = new Uint8Array([0x42, 0x54, 0x20, 0x00, 0xff]);
    expect(replaceStreamData(scope, 1, 2, content, 0, 0)).toBe(true);

    // The mutant gets the length right and every byte wrong, so a length assertion passes it.
    expect(fake.seen[0].bytes).toHaveLength(content.length);

    // AND THE ROUND-TRIP CHECK REFUSES IT. This runs the assertion the first test in this file
    // makes, against the mutant, and requires it to throw. "The mutant differs" is a weaker
    // statement than "the check catches the mutant", and only the second one is the property
    // being claimed.
    expect(() => expect(fake.seen[0].bytes).toEqual(content)).toThrow();
  });

  it("the null handle is forwarded, and is not invented on the JS side", () => {
    const fake = fakeQpdf();
    const { scope, attach } = loadBridge();
    attach(fake.module);

    const handle = scope.__burrow_qpdf_oh_new_null(1 as never) as number;

    // 77 is what the fake's `_qpdf_oh_new_null` returns. A bridge that answered 0, or that
    // made a handle up, would pass an `expect(typeof handle).toBe("number")`.
    expect(handle).toBe(77);
  });

  it("font surgery's array write reaches qpdf argument for argument", () => {
    // `qpdf_oh_set_array_item(data, oh, at, item)`, #191's one new export. It replaces a
    // `/Widths` entry or a `/Differences` name IN PLACE, so an `at` and an `item` that crossed
    // would write a handle id where an index belongs -- a valid call on the wrong slot, which
    // nothing downstream refuses. Four distinct values, so any swap is a different call.
    const fake = fakeQpdf();
    const calls: number[][] = [];
    fake.module._qpdf_oh_set_array_item = (...args: number[]) => {
      calls.push(args);
    };
    const { scope, attach } = loadBridge();
    attach(fake.module);

    const returned = scope.__burrow_qpdf_oh_set_array_item(
      3 as never,
      41 as never,
      7 as never,
      99 as never,
    );

    expect(calls).toEqual([[3, 41, 7, 99]]);
    // `void` on the C side: the verdict is latched in qpdf's error slot, which Rust drains. A
    // bridge that answered something here would be inventing a result.
    expect(returned).toBeUndefined();
  });
});
