// Every typed error the rotate tool can meet, as a sentence for a person.
//
// A PURE FUNCTION, for the reason `merge-messages.ts` gives: every branch is tested against a
// planted reply rather than only the ones a browser happens to produce, and the engine's own
// words never reach this file. The input is a `kind` and a limit name, both computed in Rust.
//
// # Why this is a second file rather than a shared one
//
// Rotate meets a different set, and the overlapping ones want different sentences. Merge has
// `InputFailed` and a file index because it takes several documents; rotate takes one, so
// there is no index and no wrapper to unwrap. Merge's `Malformed` says "remove it and try the
// rest", which is meaningless when there is only one file.
//
// The sentences are the product here, not an implementation detail — a shared `messageFor`
// with a per-tool lookup table would put them behind an indirection and make "what does this
// page say when a scan is damaged?" a question you answer by tracing code.
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
            ? `That file is ${asked}, and burrow stops at ${allowed}.`
            : "That file is larger than burrow will open.",
        next: "Nothing was read. A smaller file, or one split into parts first, will work.",
        retryable: true,
      };
    }
    case "max_pages": {
      const asked = count(failure.requested);
      const allowed = count(failure.allowed);
      return {
        title:
          asked && allowed
            ? `That document has ${asked} pages, and burrow stops at ${allowed}.`
            : "That document has more pages than burrow will open.",
        // NOT "nothing was read". `max_input_bytes` is checked from `Blob.size` before a byte
        // is read, so that branch can say it; `max_pages` is enforced after the document is in
        // the engine's memory. Telling somebody their file was never opened when it was is the
        // same overclaim the `max_memory_bytes` branch below goes out of its way to avoid.
        // Found by code review.
        next: "It was opened but not changed. Split it into smaller documents first.",
        retryable: true,
      };
    }
    case "max_duration_ms":
      return {
        title: "That took longer than burrow allows and was stopped.",
        next: "Nothing was changed. A document with fewer pages will finish.",
        retryable: true,
      };
    case "max_memory_bytes":
      return {
        // DETECTED, NOT PREVENTED (ADR 0007), and the sentence says which. "burrow stopped
        // it" would claim a bound that does not exist: the memory was already spent when
        // this was noticed.
        title: "That document used more memory than burrow allows, and was stopped.",
        next: "It had already been read by then, so the tab may be slow for a moment. Nothing was changed.",
        retryable: true,
      };
    default:
      return {
        title: "That document is past one of burrow's limits.",
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
        next: "burrow cannot open it yet. Remove the password in the app that made it, then choose it again.",
        retryable: true,
      };
    case "Malformed":
      return {
        // ALSO THE `/Rotate` CASE, deliberately. A document whose `/Rotate` is not a whole
        // number of quarter turns is malformed, and burrow refuses it rather than rounding —
        // guessing would turn a page on a value the file never carried. The sentence does not
        // distinguish the two, because "damaged" is what both are and a person cannot act on
        // the difference.
        title: "burrow could not read that file.",
        next: "It may be damaged, or not a PDF at all. Try another copy of it.",
        retryable: true,
      };
    case "Unsupported":
      return {
        title: "That PDF uses something burrow does not handle.",
        next: "Nothing was changed. Try another copy of it.",
        retryable: true,
      };
    case "LimitExceeded":
      return limitMessage(failure);
    case "InvalidArgument":
      return {
        // Reachable from the page rather than from the file: a page number past the end, or
        // an angle that is not a quarter turn. The selection box catches both before they get
        // here, so arriving at this branch means the two disagree — which is a bug in the
        // page, and saying so is more useful than blaming the document.
        title: "burrow could not make sense of that request.",
        next: "This is a bug in the page rather than a problem with your file. Reloading may clear it.",
        retryable: true,
      };
    case "OutputRejected":
      return {
        // ADR 0022. burrow produced a document, checked it against what it promised, and
        // would not hand it over. Both halves of the generic sentence are FALSE here: the
        // file is not fine, and trying again runs the same operation on the same bytes and
        // refuses in the same place -- which is why this needs its own branch rather than
        // the fallback it fell to until code review.
        //
        // The known cause is issue #61: a damaged-but-openable document that qpdf reads as
        // one page count and writes as another. So the honest next step is about the
        // DOCUMENT rather than about trying again.
        //
        // `retryable: true` even so, and the field's name is why that reads oddly: it does
        // not mean "retrying would work", it means "no deliberate gesture is needed to get
        // the page working again". `false` renders the Start again button, which clears the
        // CIRCUIT BREAKER (ADR 0015 §3) -- and the breaker has not latched, so the button
        // would do nothing and offering it would be the page lying about its own state.
        title: "burrow checked the turned document and would not hand it over.",
        next: "Your file has not been changed and nothing was sent anywhere. Some of its pages did not survive the write, so burrow refused the result rather than give you a document quietly short of a page. A copy of the file saved again from the program that made it usually works.",
        retryable: true,
      };
    case "EngineUnavailable":
      return {
        title: "burrow has stopped after several failures in a row.",
        next: "Nothing is running. Choose Start again when you are ready.",
        retryable: false,
      };
    case "Io":
    case "Internal":
    default:
      return {
        title: "Something inside burrow failed.",
        next: "Your file is fine and nothing was sent anywhere. Try again.",
        retryable: true,
      };
  }
}
