// Every typed error, as a sentence for a person.
//
// A PURE FUNCTION, so every branch is tested against a planted reply rather than only the
// ones a browser happens to produce. The engine's own words never reach this file: the
// input is a `kind`, an index and a limit name, all of them computed in Rust, and there is
// no path by which a message from qpdf or PDFium could arrive here to be rendered.
//
// That is not incidental. `apps/web/CLAUDE.md`: "Never send file content anywhere, including
// in logs and error messages. If an operation fails, report the typed error from the core,
// never the input." An engine's message carries byte offsets and object numbers, which are
// file content in every way that matters.
//
// WRITING RULES, from the design brief: errors do not apologise, and they are never vague
// about what happened. Each one says what went wrong and what to do next, in the
// interface's voice. "Sorry, something went wrong" is the thing this file exists to
// prevent.

/** The fields of a reply this module reads. Everything here is computed in Rust. */
export interface Failure {
  /** The typed variant's name, e.g. `Malformed`. Never engine prose. */
  kind: string;
  /** Which input failed, or -1. */
  failedInput?: number;
  /** What was wrong with that input, when `kind` is `InputFailed`. */
  innerKind?: string;
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
   * Which file this is about, or -1.
   *
   * So the list can mark it. A UI that had to find the file by reading the title would be
   * parsing prose, which is what `failedInput` exists to avoid.
   */
  file: number;
  /**
   * Whether a person can retry by acting on the page.
   *
   * `EngineUnavailable` is the only one that needs a deliberate gesture to clear, because
   * the circuit breaker latches on purpose (ADR 0015 §3).
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

function fileIndex(index: number | undefined): number {
  return typeof index === "number" && index >= 0 ? index : -1;
}

/** The sentence for a limit, which is the one error where the numbers help. */
function limitMessage(failure: Failure): Message {
  const file = fileIndex(failure.failedInput);
  switch (failure.limit) {
    case "max_input_bytes":
      return {
        title: `Those files come to ${mb(failure.requested)}, over the ${mb(
          failure.allowed,
        )} limit.`,
        next: "Remove some of them, or merge them in two goes and combine the results.",
        // AGGREGATE, so -1, whatever index the reply carries. The core checks the running
        // total as it walks the inputs and wraps the refusal in `InputFailed { index }`, so
        // the reply names the file the total happened to cross on -- and marking that one
        // "cannot be used" tells someone to remove a document that is perfectly fine, and
        // then refuses to merge until they do. Same inversion the deadline branch below
        // refuses, from the other direction.
        file: -1,
        retryable: true,
      };
    case "max_pages":
      return {
        title: `That would make ${failure.requested ?? "too many"} pages, over the ${
          failure.allowed ?? "limit"
        }-page limit.`,
        next: "Merge fewer files at a time.",
        // Aggregate, as above. The remedy is about the set -- "merge fewer files" -- so a
        // message carrying a file index would be arguing with its own advice.
        file: -1,
        retryable: true,
      };
    case "max_memory_bytes":
      return {
        // NOT "burrow ran out of memory". The limit DETECTS an overrun after the fact
        // (ADR 0007); saying the tab ran out would be a stronger claim than the code makes,
        // and it would send someone to close other tabs when the file is the problem.
        title: "One of those files needs more memory than burrow will spend on it.",
        next: "It is probably built in a way that expands enormously when opened. Try merging the others without it.",
        file,
        retryable: true,
      };
    case "max_duration_ms":
      return {
        title: "That merge took too long and was stopped.",
        next: "Try fewer files at once.",
        // DELIBERATELY -1, whatever the reply says. Running out of time is the operation's
        // outcome, not a file's -- blaming the document the clock happened to stop on would
        // tell someone to remove one that is perfectly fine. The core refuses to attribute
        // it and so does this.
        file: -1,
        retryable: true,
      };
    default:
      return {
        title: "That is more than burrow will take on.",
        next: "Try fewer or smaller files.",
        file,
        retryable: true,
      };
  }
}

/**
 * What to tell someone about a failed operation.
 *
 * Every `kind` the core can produce has a branch. The fallback exists because
 * `burrow_types::Error` is `#[non_exhaustive]` and a variant added upstream must arrive as
 * something a person can act on rather than as a blank.
 */
export function messageFor(failure: Failure): Message {
  // `InputFailed` is a wrapper: what is wrong is the inner kind, and WHICH file is the
  // index. Unwrapped here, once, rather than in the component.
  const kind = failure.kind === "InputFailed" ? (failure.innerKind ?? "") : failure.kind;
  const file = fileIndex(failure.failedInput);

  switch (kind) {
    case "PasswordRequired":
      return {
        title: "That PDF is password-protected.",
        next: "burrow cannot open it yet. Remove the password in the app that made it, then add it again.",
        file,
        retryable: true,
      };
    case "Malformed":
      return {
        title: "burrow could not read that file.",
        next: "It may be damaged, or not a PDF at all. Remove it and try the rest.",
        file,
        retryable: true,
      };
    case "Unsupported":
      return {
        title: "That PDF uses something burrow does not handle.",
        next: "Remove it and try the rest.",
        file,
        retryable: true,
      };
    case "LimitExceeded":
      return limitMessage(failure);
    case "InvalidArgument":
      return {
        title: "burrow could not make sense of that request.",
        next: "This is a bug in the page rather than a problem with your files. Reloading may clear it.",
        file: -1,
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
        title: "burrow checked the merged document and would not hand it over.",
        next: "Nothing was sent anywhere and none of your files has been changed. Some of the pages did not survive the merge, so burrow refused the result rather than give you a document quietly missing part of one of them. A copy of each file saved again from the program that made it usually works.",
        file: -1,
        retryable: true,
      };
    case "EngineUnavailable":
      return {
        // The breaker has latched. It does that ON PURPOSE (ADR 0015 §3) -- retrying on a
        // timer would resume a crash loop at a slower rate rather than end it -- so the way
        // out is a person deciding to spend another worker.
        title: "burrow has stopped after several failures in a row.",
        next: "Nothing is running. Choose Start again when you are ready.",
        file: -1,
        retryable: false,
      };
    case "Io":
    case "Internal":
    default:
      return {
        // NOT "an unexpected error occurred". That says nothing and sounds like an
        // apology. What a person needs to know is whether their files are the problem, and
        // here they are not.
        title: "Something inside burrow failed.",
        next: "Your files are fine and nothing was sent anywhere. Try again.",
        file: -1,
        retryable: true,
      };
  }
}
