// R10's final hop (see `delivered-digest.ts`): the page delivers a Blob only when its bytes are the
// ones the redaction module hashed. Each case is a way the hop could go wrong, and the check must
// refuse it; the one that delivers is held to the platform's own SHA-256 computed outside the
// module under test.

import { createHash } from "node:crypto";

import { describe, expect, it } from "vitest";

import { checkDelivered, type Digest } from "./delivered-digest";

const DOCUMENT = new TextEncoder().encode("%PDF-1.7\n% the redacted document\n%%EOF\n");
const hex = (bytes: Uint8Array) => createHash("sha256").update(bytes).digest("hex");
const blob = (bytes: Uint8Array) =>
  new Blob([bytes as Uint8Array<ArrayBuffer>], { type: "application/pdf" });

describe("the page delivers only the bytes the redaction module hashed (R10)", () => {
  it("delivers a Blob whose bytes are the ones the digest names", async () => {
    expect(await checkDelivered(blob(DOCUMENT), hex(DOCUMENT))).toEqual({ ok: true });
  });

  it("refuses a Blob one byte longer than the hashed bytes", async () => {
    const delivered = new Uint8Array([...DOCUMENT, 0x20]);
    expect(await checkDelivered(blob(delivered), hex(DOCUMENT))).toEqual({
      ok: false,
      kind: "OutputRejected",
      reason: "mismatch",
    });
  });

  it("refuses a Blob with one byte changed", async () => {
    const delivered = DOCUMENT.slice();
    delivered[10] ^= 0x01;
    expect((await checkDelivered(blob(delivered), hex(DOCUMENT))).ok).toBe(false);
  });

  it("refuses a Blob one byte shorter than the hashed bytes", async () => {
    expect((await checkDelivered(blob(DOCUMENT.slice(1)), hex(DOCUMENT))).ok).toBe(false);
  });

  it("refuses an empty Blob against a document's digest", async () => {
    expect((await checkDelivered(blob(new Uint8Array()), hex(DOCUMENT))).ok).toBe(false);
  });

  // A REPLY WITH A DOCUMENT AND NO USABLE DIGEST is not one the page can bind: it fails closed.
  // Each near-miss of the format `Reply::output_digest` writes is refused before anything is read.
  it.each([
    ["absent", undefined],
    ["empty", ""],
    ["upper-case", hex(DOCUMENT).toUpperCase()],
    ["63 digits", hex(DOCUMENT).slice(1)],
    ["65 digits", `${hex(DOCUMENT)}0`],
    ["not a string", Array.from(createHash("sha256").update(DOCUMENT).digest())],
    ["padded", ` ${hex(DOCUMENT)}`],
  ])("refuses a digest that is %s, without reading the Blob", async (_, digest) => {
    let read = 0;
    const counting = blob(DOCUMENT);
    const arrayBuffer = counting.arrayBuffer.bind(counting);
    counting.arrayBuffer = () => {
      read += 1;
      return arrayBuffer();
    };
    expect(await checkDelivered(counting, digest)).toEqual({
      ok: false,
      kind: "OutputRejected",
      reason: "no-digest",
    });
    expect(read).toBe(0);
  });

  it("hashes exactly the Blob's bytes with SHA-256, then zeroes its own copy", async () => {
    const seen: { algorithm: string; bytes: number[] }[] = [];
    let given: ArrayBuffer | null = null;
    const spy: Digest = async (algorithm, data) => {
      seen.push({ algorithm, bytes: Array.from(new Uint8Array(data)) });
      given = data;
      return crypto.subtle.digest(algorithm, data);
    };
    expect(await checkDelivered(blob(DOCUMENT), hex(DOCUMENT), spy)).toEqual({ ok: true });
    expect(seen).toEqual([{ algorithm: "SHA-256", bytes: Array.from(DOCUMENT) }]);
    // #199's habit: the page's read-back copy does not outlive the check.
    expect(Array.from(new Uint8Array(given!)).every((b) => b === 0)).toBe(true);
  });

  it("does not deliver when the platform digest fails", async () => {
    const failing: Digest = () => Promise.reject(new Error("no digest"));
    await expect(checkDelivered(blob(DOCUMENT), hex(DOCUMENT), failing)).rejects.toThrow();
  });
});
