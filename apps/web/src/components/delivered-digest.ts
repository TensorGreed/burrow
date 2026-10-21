// R10's final hop on the web (ADR 0006; #206's web half, cut to the runtime digest by the owner's
// decision 56, 2026-10-21).
//
// R10: the bytes verification examines are proven identical to the bytes emitted. "Emitted" is the
// Rust buffer the operation returns (owner, 2026-10-19), and the native tests prove that buffer is
// the one verification examined (`redact_verify_same_bytes`). What they cannot see is the web's
// last hop: out of the wasm heap, through the worker's reply, into the Blob this page hands over.
// The redaction module takes the SHA-256 of the returned buffer in Rust (`Reply::output_digest`),
// the reply carries it as `outputDigest`, and this re-takes it over the bytes of the very Blob the
// download link is made from. The page hands the file over only when the two agree.
//
// WHAT IT BINDS: the delivered bytes to the bytes Rust returned. A byte changed, added or lost
// anywhere between the `Vec` and the Blob -- in `takeOutput`, in the worker's `drainReply`, in
// structured clone, in the page's own handling -- fails it, and nothing is handed over.
//
// WHAT IT DOES NOT BIND, stated:
// - That the buffer Rust hashed is the one verification examined. That is the native half of R10,
//   held by `redact_verify_same_bytes`, not here.
// - A worker that lies about both. The digest and the output travel in one reply from one worker;
//   a compromised worker could send a different document with that document's digest. This is an
//   integrity check of the hop, not an authentication of the worker -- the worker's integrity is
//   the bundle pin's (SRI) to hold.
// - What happens after the hand-over: the download is the browser's.
//
// THE COST. Reading a Blob back puts one copy of the redacted document in the page's heap, which
// `tool-delivery.ts` otherwise avoids; it is zeroed as soon as it is hashed. `crypto.subtle.digest`
// takes its own copy of its input, which nothing here can zero. The redacted document is the
// output, not the secret -- the secret is what redaction removed -- but #199's habit holds anyway.

/** A digest as `Reply::output_digest` writes it: 64 lowercase hex digits. */
const HEX_SHA256 = /^[0-9a-f]{64}$/;

/** Why a delivery was refused. Both are an `OutputRejected` to the person. */
export type DeliveryRefusal = "no-digest" | "mismatch";

/** The check's answer: deliver, or a typed refusal and nothing handed over. */
export type DeliveryCheck =
  { ok: true } | { ok: false; kind: "OutputRejected"; reason: DeliveryRefusal };

/** The one primitive used, injected so the unit test can see what it was given. */
export type Digest = (algorithm: "SHA-256", data: ArrayBuffer) => Promise<ArrayBuffer>;

const platformDigest: Digest = (algorithm, data) => crypto.subtle.digest(algorithm, data);

/**
 * Whether `blob` -- the Blob the page is about to hand over -- is the document whose SHA-256 the
 * redaction module reported as `digest`.
 *
 * A missing or malformed digest refuses: a reply that carries a document and no digest is not one
 * this page can bind, so it is not delivered (fail closed).
 */
export async function checkDelivered(
  blob: Blob,
  digest: unknown,
  sha256: Digest = platformDigest,
): Promise<DeliveryCheck> {
  if (typeof digest !== "string" || !HEX_SHA256.test(digest)) {
    return { ok: false, kind: "OutputRejected", reason: "no-digest" };
  }
  const bytes = await blob.arrayBuffer();
  let taken: ArrayBuffer;
  try {
    taken = await sha256("SHA-256", bytes);
  } finally {
    new Uint8Array(bytes).fill(0);
  }
  const hex = Array.from(new Uint8Array(taken), (b) => b.toString(16).padStart(2, "0")).join("");
  return hex === digest ? { ok: true } : { ok: false, kind: "OutputRejected", reason: "mismatch" };
}
