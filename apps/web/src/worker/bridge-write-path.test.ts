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
};

/**
 * An Emscripten module with a real heap, a bump allocator, and a free that is checked.
 *
 * The allocator does not reuse, so a pointer freed twice or used after a free is visible rather
 * than absorbed — the shape the `free(0xc0ffee)` defect had.
 */
function fakeQpdf(heapBytes = 1 << 16): Fake {
  const buffer = new ArrayBuffer(heapBytes);
  const seen: Seen[] = [];
  const outstanding = new Set<number>();
  // Never 0: the bridge treats 0 as an allocation failure, so an allocator handing it out would
  // make the refusal path fire on a perfectly good call.
  let next = 8;

  const module: Record<string, unknown> = {
    HEAPU8: new Uint8Array(buffer),
    HEAPU32: new Uint32Array(buffer),
    _malloc: (n: number) => {
      const at = next;
      next += Math.max(n, 1) + 8;
      if (next > heapBytes) {
        return 0;
      }
      outstanding.add(at);
      return at;
    },
    _free: (ptr: number) => {
      if (!outstanding.delete(ptr)) {
        throw new Error(`free of a pointer that is not outstanding: ${ptr}`);
      }
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
  return { module, seen, live: () => outstanding.size };
}

/** Evaluate the bridge the way the bundler concatenates it, and hand back its entry points. */
function loadBridge(qpdfSource?: string): {
  scope: Record<string, (...args: never[]) => unknown>;
  attach: (module: unknown) => void;
} {
  const common = readFileSync(join(workerDir, "bridge-common.js"), "utf8");
  const qpdf = qpdfSource ?? readFileSync(join(workerDir, "bridge-qpdf.js"), "utf8");
  const scope: Record<string, (...args: never[]) => unknown> = {};
  // `__burrow_attach` is a bundle-scope function rather than a `self.` global, so the loader
  // returns it explicitly. Everything else the bridge publishes lands on `scope`.
  const load = new Function("self", `${common}\n${qpdf}\nreturn { attach: __burrow_attach };`);
  const { attach } = load(scope) as { attach: (module: unknown) => void };
  return { scope, attach };
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
    // `-sALLOW_MEMORY_GROWTH=1` replaces the buffer and detaches every existing view. This
    // models the growth inside `_malloc` and asserts the copy still lands: a bridge that
    // hoisted `module.HEAPU8` above the malloc would write into the detached view and throw.
    const fake = fakeQpdf();
    const grown = new ArrayBuffer(1 << 17);
    const original = fake.module._malloc as (n: number) => number;
    fake.module._malloc = (n: number) => {
      const at = original(n);
      fake.module.HEAPU8 = new Uint8Array(grown);
      fake.module.HEAPU32 = new Uint32Array(grown);
      return at;
    };

    const { scope, attach } = loadBridge();
    attach(fake.module);
    const content = new Uint8Array([9, 8, 7, 6]);

    expect(replaceStreamData(scope, 1, 2, content, 0, 0)).toBe(true);
    expect(fake.seen[0].bytes).toEqual(content);
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
});
