// Every typed error the split tool can meet, as a sentence for a person.
//
// A PURE FUNCTION, for the reason `merge-messages.ts` gives: every branch is tested against a
// planted reply rather than only the ones a browser happens to produce, and the engine's own
// words never reach this file.
//
// # Why a fourth file rather than a shared one
//
// The argument `reorder-messages.ts` makes, one operation on, and split is the case that makes
// it hardest to argue with. Two kinds say something here that they say nowhere else:
//
//   * `Unsupported` is, on this operation, almost always ONE thing: the document uses layers.
//     `/reorder-pdf` answers it with "uses something burrow does not handle", which is true and
//     useless — ADR 0019 §4 requires this page to give the REASON, because "layered documents
//     are refused" reads as a bug without it and as a deliberate limit with it. The page prose
//     says it; a refusal that then shrugged would be the page contradicting itself.
//   * `OutputRejected` has to name the PART. A fifty-way split that refuses on part forty-nine
//     hands over nothing (ADR 0023 §3), and "burrow checked the document" is the wrong number
//     of documents.
//
// What is genuinely identical is identical: the limit sentences are about `Limits`, not about
// the operation, and they are the same words as the other three pages.
//
// WRITING RULES, from the design brief: errors do not apologise, and are never vague about
// what happened. Each says what went wrong and what to do next, in the interface's voice.

/** The fields of a reply this module reads. Everything here is computed in Rust. */
export interface Failure {
  /** The typed variant's name, e.g. `Malformed`. Never engine prose. */
  kind: string;
  /** The ceiling that was hit, e.g. `max_pages`. */
  limit?: string;
  /** What was asked for, and what was allowed. Strings: these are `u64`. */
  requested?: string;
  allowed?: string;
}

export interface Message {
  /** One line, said plainly. */
  title: string;
  /** What to do next. Empty when there is genuinely nothing to do. */
  next: string;
  /**
   * Whether a person can retry by acting on the page.
   *
   * `EngineUnavailable` is the only one that needs a deliberate gesture, because the circuit
   * breaker latches on purpose (ADR 0015 §3).
   */
  retryable: boolean;
}

/** Human numbers for a byte count, without a dependency. */
function mb(bytes: string | undefined): string {
  const n = Number(bytes ?? "0");
  if (!Number.isFinite(n) || n <= 0) return "";
  const value = n / (1024 * 1024);
  return value >= 10 ? `${Math.round(value)} MB` : `${value.toFixed(1)} MB`;
}

/** A plain count, or an empty string when the number is not usable. */
function count(value: string | undefined): string {
  const n = Number(value ?? "0");
  if (!Number.isFinite(n) || n <= 0) return "";
  return n.toLocaleString("en");
}

/**
 * Which ceiling was reached, and what a person can do about it.
 *
 * BOTH NUMBERS, whenever the core gave them. "Too large" without a size is a refusal a person
 * cannot act on: they do not know whether to remove one page or ninety.
 */
function limitMessage(failure: Failure): Message {
  switch (failure.limit) {
    case "max_input_bytes": {
      const asked = mb(failure.requested);
      const allowed = mb(failure.allowed);
      return {
        title:
          asked && allowed
            ? `That file is ${asked}, and Not Only PDF stops at ${allowed}.`
            : "That file is larger than Not Only PDF will open.",
        next: "Nothing was read. A smaller file will work.",
        retryable: true,
      };
    }
    case "max_pages": {
      const asked = count(failure.requested);
      const allowed = count(failure.allowed);
      return {
        title:
          asked && allowed
            ? `That document has ${asked} pages, and Not Only PDF stops at ${allowed}.`
            : "That document has more pages than Not Only PDF will open.",
        // NOT "nothing was read": `max_input_bytes` is checked from `Blob.size` before a byte
        // is read and can say that; `max_pages` is enforced once the document is in the
        // engine's memory.
        next: "It was opened but not split.",
        retryable: true,
      };
    }
    case "max_duration_ms":
      return {
        // SPLIT IS CHECKPOINTED BETWEEN PARTS, not inside one (ADR 0023 §6), so a long
        // document can be stopped part-way through producing parts -- and none of them is
        // delivered. Saying "nothing was changed" is true and is not the whole answer.
        title: "That took longer than Not Only PDF allows and was stopped.",
        next: "No parts were handed over, and the file on your computer is untouched. Fewer parts, or a smaller document, will finish.",
        retryable: true,
      };
    case "max_memory_bytes":
      return {
        // DETECTED, NOT PREVENTED (ADR 0007), and the sentence says which. "burrow stopped
        // it" would claim a bound that does not exist: the memory was already spent when
        // this was noticed.
        title: "That document used more memory than Not Only PDF allows, and was stopped.",
        next: "It had already been read by then, so the tab may be slow for a moment. Nothing was changed.",
        retryable: true,
      };
    default:
      return {
        title: "That document is past one of Not Only PDF's limits.",
        next: "Nothing was changed. A smaller document will work.",
        retryable: true,
      };
  }
}

/**
 * The sentence for one failure.
 *
 * Every `kind` the core can produce on this path has a branch. The fallback exists because
 * `burrow_types::Error` is `#[non_exhaustive]`, and a variant added upstream must arrive as
 * something a person can act on rather than as a blank.
 */
export function messageFor(failure: Failure): Message {
  switch (failure.kind) {
    case "PasswordRequired":
      return {
        title: "That PDF is password-protected.",
        next: "Not Only PDF cannot open it yet. Remove the password in the app that made it, then choose it again.",
        retryable: true,
      };
    case "Malformed":
      return {
        title: "Not Only PDF could not read that file.",
        next: "It may be damaged, or not a PDF at all. Try another copy of it.",
        retryable: true,
      };
    case "Unsupported":
      return {
        // THE REFUSAL THIS PAGE IS ABOUT, and the reason is the load-bearing half.
        // ADR 0019 §4: "a page that said only 'documents with layers cannot be split' would
        // read as a bug". The core's own words for it are a fixed string about optional
        // content, and this says the same thing without echoing them — the binding is not
        // guaranteed to carry the detail, and a sentence that depends on it would be blank
        // when it does not.
        //
        // HEDGED ON PURPOSE. Layers are the only `Unsupported` split produces today, but the
        // variant is not reserved for them, so this says "usually" rather than asserting a
        // cause the page cannot see.
        title: "Not Only PDF will not split that document.",
        next: "This is usually a document that uses layers. Whether a layer is hidden is recorded for the document as a whole, so Not Only PDF cannot carry that setting into a part of it — and a hidden layer that arrived visible in one of the parts would be worse than refusing. Nothing was changed.",
        retryable: true,
      };
    case "LimitExceeded":
      return limitMessage(failure);
    case "InvalidArgument":
      return {
        // `resolveCuts` refuses every bad cut list beside the box, with the page number, so
        // arriving here means the page and the core disagree about the document. That is a
        // bug in the page, and saying so is more useful than blaming the file.
        title: "Not Only PDF could not make sense of where to cut.",
        next: "This looks like a bug in the page rather than a problem with your file — reloading may clear it.",
        retryable: true,
      };
    case "OutputRejected":
      return {
        // ADR 0022, and ADR 0023 §3 for the shape of the answer. Each part is verified
        // before it is posted, so a refusal means one part did not match what was promised
        // for it — and NOTHING is delivered, because a subset of the parts is not a
        // partition. Both halves of the generic sentence would be false: the file is not
        // fine, and trying again runs the same operation on the same bytes.
        title: "Not Only PDF checked the parts it made and would not hand them over.",
        next: "Your file has not been changed and nothing was sent anywhere. One part did not contain the pages it was supposed to, so Not Only PDF refused the whole split rather than give you parts that do not add up to your document. A copy of the file saved again from the program that made it usually works.",
        retryable: true,
      };
    case "EngineUnavailable":
      return {
        title: "Not Only PDF has stopped after several failures in a row.",
        next: "Nothing is running. Choose Start again when you are ready.",
        retryable: false,
      };
    case "Io":
    case "Internal":
    default:
      return {
        title: "Something inside Not Only PDF failed.",
        next: "Your file is fine and nothing was sent anywhere. No parts were handed over. Try again.",
        retryable: true,
      };
  }
}
