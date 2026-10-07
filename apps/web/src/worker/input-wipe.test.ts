// #199, the JS-heap half: every buffer holding a person's input or content qpdf decoded is
// zeroed before it is released.
//
// `apps/web/CLAUDE.md` states this as an invariant a review checks, because the JS heap cannot
// be inspected reliably from a test. What CAN be tested is that the code zeroes the buffers it
// holds, at the moment it lets go of them, which is what this file does:
//
// - each worker zeroes its copy of the input and the password once the Rust call has returned,
//   on success, on a refusal raised after the read, and when the call throws;
// - merge zeroes each per-file copy when it drops it, and the flat buffer after the call;
// - each worker asks the bridge to wipe what it handed out;
// - the bridge zeroes qpdf's own buffer of decoded content before freeing it, and zeroes the copy
//   it handed to Rust once that copy has been consumed.
//
// What it cannot see, stated rather than implied: copies the engine or wasm-bindgen makes
// elsewhere, the person's own `File`, which is out of scope (#199, 2026-09-26), and Rust's
// copies of decoded content, which are core's (#199's third PR).

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import { describe, expect, it } from "vitest";
import { MAIN, REDACT_MAIN, load, replyShape } from "./test-scope";

const limits = {
  maxInputBytes: 1,
  maxMemoryBytes: 1,
  maxDurationMs: 1,
  maxPages: 1,
  maxPixels: 1,
};

/** A Blob stand-in whose bytes stay reachable, so a case can read them after the worker is done. */
function blob(bytes: number[]) {
  const buffer = new Uint8Array(bytes).buffer;
  return {
    blob: { size: bytes.length, arrayBuffer: async () => buffer },
    view: () => [...new Uint8Array(buffer)],
  };
}

/** Replace one op's stub with one that records what it was handed, and checks it was intact. */
function capture(
  scope: Record<string, unknown>,
  op: string,
  behave: () => unknown = () => ({ ...replyShape(), pages: 1 }),
) {
  const seen: { args: unknown[]; intactAtCall: boolean }[] = [];
  const wb = scope["wasm_bindgen"] as Record<string, unknown>;
  wb[op] = (...args: unknown[]) => {
    const arrays = args.filter((a): a is Uint8Array => a instanceof Uint8Array);
    seen.push({ args, intactAtCall: arrays.every((a) => a.some((b) => b !== 0)) });
    return behave();
  };
  return seen;
}

const zero = (a: unknown) => [...(a as Uint8Array)].every((b) => b === 0);

describe("#199: the base worker zeroes the input and the password once Rust has them", () => {
  it("on success", async () => {
    const w = load(MAIN);
    w.releaseQpdf();
    const seen = capture(w.scope, "page_count");
    const input = blob([37, 80, 68, 70]);
    await w.send({
      id: 1,
      op: "page_count",
      blob: input.blob,
      password: new Uint8Array([7, 7]).buffer,
      limits,
    });

    expect(seen).toHaveLength(1);
    expect(seen[0]!.intactAtCall, "the bytes were zeroed BEFORE Rust read them").toBe(true);
    expect(zero(seen[0]!.args[0]), "the input survived the operation").toBe(true);
    expect(zero(seen[0]!.args[1]), "the password survived the operation").toBe(true);
    expect(
      input.view().every((b) => b === 0),
      "the ArrayBuffer behind the input survived",
    ).toBe(true);
    expect(w.wipes(), "the bridge's hand-outs were not wiped").toBe(1);
  });

  it("when the Rust call throws", async () => {
    const w = load(MAIN);
    w.releaseQpdf();
    const seen = capture(w.scope, "page_count", () => {
      throw new Error("a trap");
    });
    await w.send({ id: 1, op: "page_count", blob: blob([1, 2, 3]).blob, limits });

    expect(
      w.posted.some((m) => m["kind"] === "Internal"),
      "the throw did not reach the protocol",
    ).toBe(true);
    expect(zero(seen[0]!.args[0]), "a throwing call left the input intact").toBe(true);
    expect(w.wipes()).toBe(1);
  });

  it("on a refusal raised after the file was read", async () => {
    const w = load(MAIN);
    w.releaseQpdf();
    const input = blob([9, 9, 9]);
    await w.send({ id: 1, op: "rotate", blob: input.blob, pages: [1.5], degrees: 90, limits });

    expect(w.posted.some((m) => m["kind"] === "InvalidArgument")).toBe(true);
    expect(
      input.view().every((b) => b === 0),
      "a refused request left the input intact",
    ).toBe(true);
  });

  it("merge: every per-file copy, and the flat buffer", async () => {
    const w = load(MAIN);
    w.releaseQpdf();
    const seen = capture(w.scope, "merge");
    const a = blob([1, 2]);
    const b = blob([3, 4, 5]);
    await w.send({ id: 1, op: "merge", blobs: [a.blob, b.blob], limits });

    expect(seen[0]!.intactAtCall).toBe(true);
    expect([...(seen[0]!.args[0] as Uint8Array)]).toHaveLength(5);
    expect(zero(seen[0]!.args[0]), "the flat buffer survived").toBe(true);
    expect(
      a.view().every((x) => x === 0),
      "the first file's copy survived",
    ).toBe(true);
    expect(
      b.view().every((x) => x === 0),
      "the second file's copy survived",
    ).toBe(true);
  });
});

describe("#199: the redaction worker zeroes the document and the password", () => {
  it("once the redaction has returned", async () => {
    const w = load(REDACT_MAIN);
    w.releaseQpdf();
    const seen = capture(w.scope, "redact");
    const input = blob([83, 69, 67, 82, 69, 84]);
    await w.send({
      id: 1,
      op: "redact",
      blob: input.blob,
      page: 1,
      covered: [1],
      region: { left: 0, top: 0, width: 1, height: 1 },
      password: new Uint8Array([7]).buffer,
      limits,
    });

    expect(seen).toHaveLength(1);
    expect(seen[0]!.intactAtCall).toBe(true);
    expect(
      input.view().every((b) => b === 0),
      "the document being redacted survived",
    ).toBe(true);
    expect(zero(seen[0]!.args[4]), "the password survived").toBe(true);
    expect(w.wipes()).toBe(1);
  });
});

describe("#199: the qpdf bridge wipes decoded content", () => {
  const source = ["bridge-common.js", "bridge-qpdf.js"]
    .map((f) => readFileSync(join(import.meta.dirname, f), "utf8"))
    .join("\n");

  /** A qpdf module whose decoded content is a known secret, and whose frees are recorded. */
  function bridge() {
    const heap = new ArrayBuffer(1 << 12);
    const u8 = new Uint8Array(heap);
    const secret = [83, 69, 67, 82, 69, 84];
    let next = 64;
    const atFree: { ptr: number; bytes: number[] }[] = [];
    const module = {
      HEAPU8: u8,
      HEAPU32: new Uint32Array(heap),
      _malloc: (n: number) => {
        const at = next;
        next += n + 8;
        return at;
      },
      _free: () => {},
      _qpdf_oh_get_page_content_data: (_d: number, _p: number, bufp: number, lenp: number) => {
        const buf = module._malloc(secret.length);
        u8.set(secret, buf);
        module.HEAPU32[bufp >>> 2] = buf;
        module.HEAPU32[lenp >>> 2] = secret.length;
      },
      _qpdf_oh_get_stream_data: (
        d: number,
        oh: number,
        _level: number,
        filteredp: number,
        bufp: number,
        lenp: number,
      ) => {
        module._qpdf_oh_get_page_content_data(d, oh, bufp, lenp);
        module.HEAPU32[filteredp >>> 2] = 1;
      },
      _qpdf_oh_free_buffer: (bufp: number) => {
        const ptr = module.HEAPU32[bufp >>> 2]!;
        atFree.push({ ptr, bytes: [...u8.slice(ptr, ptr + secret.length)] });
      },
    };
    const scope: Record<string, unknown> = {};
    scope["self"] = scope;
    runInNewContext(`${source}\n;__burrow_attach(module);`, Object.assign(scope, { module }));
    const call = (name: string) => (scope[name] as (...a: number[]) => Uint8Array)(1, 1);
    return {
      decoded: {
        oh_page_content: () => call("__burrow_qpdf_oh_page_content"),
        oh_stream_data: () => call("__burrow_qpdf_oh_stream_data"),
      } as Record<string, () => Uint8Array>,
      copyOut: () => {
        u8.set(secret, 2048);
        return (scope["__burrow_qpdf_copy_out"] as (p: number, n: number) => Uint8Array)(
          2048,
          secret.length,
        );
      },
      wipe: () => (scope["__burrow_wipe_handed_out"] as () => void)(),
      atFree,
    };
  }

  describe.each(["oh_page_content", "oh_stream_data"])("%s", (name) => {
    it("zeroes qpdf's buffer before it is freed", () => {
      const b = bridge();
      const copy = b.decoded[name]!();
      expect([...copy], "the copy handed to Rust is the content").toEqual([83, 69, 67, 82, 69, 84]);
      expect(b.atFree).toHaveLength(1);
      expect(
        b.atFree[0]!.bytes.every((x) => x === 0),
        "qpdf's buffer was freed with the content in it",
      ).toBe(true);
    });

    it("zeroes the copy it handed out, at the next hand-out and at the end", () => {
      const b = bridge();
      const first = b.decoded[name]!();
      const second = b.decoded[name]!();
      expect(zero(first), "an earlier hand-out survived the next one").toBe(true);
      expect(zero(second), "the latest hand-out was zeroed before Rust could copy it").toBe(false);
      b.wipe();
      expect(zero(second), "the last hand-out survived the operation's end").toBe(true);
    });
  });

  it("zeroes the written document's copy too", () => {
    const b = bridge();
    const out = b.copyOut();
    expect(zero(out)).toBe(false);
    b.wipe();
    expect(zero(out), "copy_out's hand-out survived the operation's end").toBe(true);
  });
});
