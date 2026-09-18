// Every typed error the compress tool can meet, and — unlike the other four — every
// successful RESULT, as sentences for a person.
//
// A PURE FUNCTION, for the reason `merge-messages.ts` gives: every branch is tested against a
// planted reply rather than only the ones a browser happens to produce, and the engine's own
// words never reach this file.
//
// # Why this file also renders success, where the others do not
//
// The other four tools have one successful answer: here is your document. `compress` has
// three, and two of them are the ones a person most needs explained.
//
// Spike 0005 measured what the operation is worth, and the spread is two orders of magnitude:
// 85.6% on a form, 12.0% on a text report, **0.15% on a scan**, 0.12% on a photo-heavy
// document, median 16.0%. The person arriving at a page called "Compress PDF" with a 12 MB
// scan is the modal visitor, and they will get 12 MB back.
//
// A number alone, in that case, reads as a broken tool. So the result is a sentence that says
// what happened AND why, and the why is specific: burrow changes how a PDF stores its
// structure, and a scan is a stack of images whose data is already compressed — there is
// nothing left to reorganise, and the only way to make it smaller would be to make it look
// worse, which this tool will not do.
//
// WRITING RULES, from the design brief: errors do not apologise, and are never vague. Neither
// does a disappointing result — "this file is already efficiently stored" is a finding, and it
// is said as one.

/** The fields of a failing reply this module reads. Everything here is computed in Rust. */
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

/**
 * Below this, a saving is reported as "not worth it" rather than as a result.
 *
 * **One percent, and the number comes from the measurement rather than from taste.** Spike
 * 0005 ran the operation over qpdf's own 618-file corpus: 60 of them saved under 1%, the
 * scanned fixture came in at 0.15% and the photo-heavy one at 0.12%, while a text report was
 * 12.0% and a form 85.6%. Under one percent nothing structural happened; above it, something
 * did.
 *
 * It changes WORDING ONLY. The document is still offered whenever there is one — a person who
 * wants their 0.15% may have it, and the page does not decide for them. What the threshold
 * prevents is the page presenting a rounding error as an achievement.
 */
export const NEGLIGIBLE_SAVING = 0.01;

/**
 * Human bytes, without a dependency. The other tools' `mb`, with two differences it needs.
 *
 * THIS PAGE IS THE ONE THAT SHOWS SIZES AS ITS RESULT, so a size that renders as nothing is
 * not a tidy degradation here, it is a hole where the measurement should be: "its version came
 * to  against your 1.2 MB" is worse than either number. So a non-positive count renders as
 * `0 bytes` rather than as an empty string, and only a genuinely unreadable one is empty --
 * which the caller checks for separately.
 *
 * AND SMALL FILES GET KB. Everything under a tenth of a megabyte is "0.1 MB" or "0.0 MB"
 * otherwise, and a person compressing a 40 KB file would watch it turn into 0.0 MB twice and
 * conclude the tool had eaten it.
 */
function mb(bytes: string | undefined): string {
  const n = Number(bytes ?? "0");
  if (!Number.isFinite(n) || n < 0) return "";
  if (n === 0) return "0 bytes";
  const value = n / (1024 * 1024);
  if (value < 0.1) return `${Math.max(1, Math.round(n / 1024))} KB`;
  return value >= 10 ? `${Math.round(value)} MB` : `${value.toFixed(1)} MB`;
}

/** A plain count, or an empty string when the number is not usable. */
function count(value: string | undefined): string {
  const n = Number(value ?? "0");
  if (!Number.isFinite(n) || n <= 0) return "";
  return n.toLocaleString("en");
}

/** How a result should be presented. Three, because the operation has three answers. */
export type ResultKind = "smaller" | "negligible" | "unchanged";

export interface Result {
  kind: ResultKind;
  /** The headline, carrying both sizes where they say something. */
  title: string;
  /** Why it came out this way. Empty for the case that needs no explaining. */
  detail: string;
  /** Whether there is a document to offer. */
  hasDocument: boolean;
  /** The saving as a fraction, for a caller that wants to render it itself. */
  saved: number;
}

/**
 * WHY A DOCUMENT DID NOT SHRINK, in a person's words.
 *
 * Shared between the two disappointing outcomes because the cause is the same one, and it is
 * the sentence this whole page exists to get right: a scan is the modal input, 0.15% is the
 * measured answer, and a bare number there reads as a tool that is broken rather than as a
 * document that was already efficient.
 *
 * It says what burrow DOES do, so the limit is legible rather than mysterious — and it says
 * what burrow will not do, because "we could have made it smaller by damaging it" is the part
 * a person would otherwise wonder about.
 */
const WHY_NOT_SMALLER =
  "Not Only PDF makes a PDF smaller by changing how it stores its structure — packing thousands of " +
  "small pieces of bookkeeping together and dropping what nothing points at. It never touches " +
  "your pages. A document that is mostly scanned or photographed pages has almost no structure " +
  "to reorganise: the picture data is already compressed, and the only way to shrink it further " +
  "would be to make it look worse. Not Only PDF will not do that.";

/**
 * What to say about a successful compression.
 *
 * `producedBytes` rather than the document's length, deliberately: on the unchanged branch
 * there is no document to measure, and this is the only record of what burrow's own version
 * weighed (ADR 0025 §3). Both numbers ride on the reply so this needs no second round trip.
 *
 * NOTE THE VOCABULARY, which is load-bearing rather than stylistic. Nothing in this file may
 * say burrow "re-encoded" anything: the page's central claim is that it does NOT re-encode,
 * and the word names precisely the lossy thing it refuses to do. burrow REWRITES a document --
 * it stores the same pages a different way. An earlier draft of the unchanged branch below said
 * "burrow re-encoded it", which told a person their file had been through exactly the treatment
 * the rest of the page promises it was spared, on the one outcome where nothing happened at
 * all.
 */
export function resultFor(reply: {
  originalBytes: string;
  producedBytes: string;
  hasDocument: boolean;
}): Result {
  const original = Number(reply.originalBytes);
  const produced = Number(reply.producedBytes);

  // A REPLY THAT CANNOT BE READ IS NOT AN ACHIEVEMENT. Both are `u64` strings from Rust, so
  // this is unreachable today; it exists so a future shape change degrades to the honest
  // answer rather than to `NaN%`.
  if (!Number.isFinite(original) || !Number.isFinite(produced) || original <= 0) {
    return {
      kind: reply.hasDocument ? "smaller" : "unchanged",
      title: reply.hasDocument
        ? "Your compressed document is ready."
        : "This file is already efficiently stored.",
      detail: reply.hasDocument ? "" : WHY_NOT_SMALLER,
      hasDocument: reply.hasDocument,
      saved: 0,
    };
  }

  const saved = (original - produced) / original;

  // NOT SMALLER AT ALL. The core returned no document, by design -- it refuses to hand back
  // something no better than what it was given, and returns two counts instead of a copy of
  // the input (ADR 0025 §3). "Rewrote", not "re-encoded": see the note above.
  //
  // THIS IS NOT A FAILURE and is not presented as one. Nothing went wrong; the file was
  // already efficiently stored, which is a fact about the file.
  if (!reply.hasDocument) {
    return {
      kind: "unchanged",
      title: "This file is already efficiently stored.",
      detail: `Not Only PDF rewrote it and its version came to ${mb(
        reply.producedBytes,
      )} against your ${mb(reply.originalBytes)}, so it kept yours. Nothing was re-encoded on the way — the rewrite simply had nothing left to pack. ${WHY_NOT_SMALLER}`,
      hasDocument: false,
      saved,
    };
  }

  // SMALLER, BUT BY NOTHING WORTH HAVING. The scan case: measured at 0.15%. Offered anyway --
  // see `NEGLIGIBLE_SAVING` -- but not dressed up.
  if (saved < NEGLIGIBLE_SAVING) {
    return {
      kind: "negligible",
      title: `${mb(reply.originalBytes)} → ${mb(reply.producedBytes)}. A saving of ${percent(
        saved,
      )}, which is not worth replacing your file for.`,
      detail: WHY_NOT_SMALLER,
      hasDocument: true,
      saved,
    };
  }

  return {
    kind: "smaller",
    title: `${mb(reply.originalBytes)} → ${mb(reply.producedBytes)}. ${percent(saved)} smaller.`,
    // THE LOSSLESS CLAIM, ON THE HAPPY PATH TOO. It is the thing that distinguishes this tool
    // from the ones that shrink a PDF by resampling its images, and somebody who just watched
    // a file get a third smaller is entitled to know which of those happened.
    // "UNALTERED", NOT "BYTE-FOR-BYTE", and the distinction is the repository's own: the one
    // lever is object-stream generation, and qpdf's writer will flate a content stream that
    // arrived uncompressed -- so the DECODED instructions are identical and the bytes need not
    // be. `compress_keeps_everything.rs` says the same thing about its own assertion, which is
    // a substring search rather than a whole-body comparison, "the same wording
    // rotate_keeps_everything.rs settled on after code review caught the overclaim there".
    // Saying byte-for-byte here would be that overclaim arriving on a page, in the one sentence
    // a person leans on to know their images survived.
    detail:
      "Nothing was re-encoded: every page's contents come through unaltered, and no image, " +
      "font or drawing was touched. Only the way the document stores them has changed.",
    hasDocument: true,
    saved,
  };
}

/**
 * A saving as a percentage a person can read.
 *
 * One decimal below ten percent, none above: "0.1%" is the difference between "nothing
 * happened" and "something did", while "31.4%" claims a precision the number does not have.
 */
export function percent(saved: number): string {
  const value = saved * 100;
  if (!Number.isFinite(value) || value <= 0) return "0%";
  return value >= 10 ? `${Math.round(value)}%` : `${value.toFixed(1)}%`;
}

/**
 * Which ceiling was reached, and what a person can do about it.
 *
 * BOTH NUMBERS, whenever the core gave them. "Too large" without a size is a refusal a person
 * cannot act on.
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
            ? `That document has ${asked} pages, and Not Only PDF stops at ${allowed}.`
            : "That document has more pages than Not Only PDF will open.",
        // NOT "nothing was read": `max_pages` is enforced after the document is in the
        // engine's memory, unlike `max_input_bytes`, which is checked from `Blob.size`.
        next: "It was opened but not changed. Split it into smaller documents first.",
        retryable: true,
      };
    }
    case "max_duration_ms":
      return {
        title: "That took longer than Not Only PDF allows and was stopped.",
        // WORTH SAYING HERE MORE THAN ANYWHERE ELSE. Compression is a single engine call and
        // the most expensive one burrow makes; `max_duration_ms` is cooperative and cannot
        // interrupt it, so on the web it is the worker watchdog that ends this (ADR 0015).
        // Either way the person's file is untouched, which is the part they need.
        next: "Your file was not changed. A document with fewer pages will finish.",
        retryable: true,
      };
    case "max_memory_bytes":
      return {
        // DETECTED, NOT PREVENTED (ADR 0007), and the sentence says which.
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
 *
 * **A document that did not shrink is NOT here.** It is a success — see `resultFor` — and
 * routing it through this function is the mistake this comment exists to prevent.
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
        // THE VARIANT CANNOT TELL A DAMAGED FILE FROM A SHAPE BURROW DOES NOT UNDERSTAND, so
        // this no longer asserts the first. Measured on `/split-pdf`: an ordinary 11 MB course
        // PDF that qpdf opened, counted and compressed was reported as damaged because burrow's
        // own lexer met a byte inside an embedded font program (#112). This page shows the
        // document's page count before the operation runs, so "we could not read it" is a
        // claim the page has already disproved on screen.
        title: "Not Only PDF could not finish reading that document.",
        next: "If it opens elsewhere, the problem is here rather than in your file — that has happened, and it is worth reporting. Nothing was changed.",
        retryable: true,
      };
    case "Unsupported":
      return {
        title: "That PDF uses something Not Only PDF does not handle.",
        next: "Nothing was changed. Try another copy of it.",
        retryable: true,
      };
    case "LimitExceeded":
      return limitMessage(failure);
    case "InvalidArgument":
      return {
        // Compression takes no selection at all -- no page list, no angle, no cut -- so there
        // is nothing a person can ask for wrongly. Reaching this branch means the page sent
        // something the core would not accept, which is a bug in the page.
        title: "Not Only PDF could not make sense of that request.",
        next: "This is a bug in the page rather than a problem with your file. Reloading may clear it.",
        retryable: true,
      };
    case "OutputRejected":
      return {
        // ADR 0022. burrow produced a document, checked it against what it promised, and would
        // not hand it over. The known cause is issue #61: a damaged-but-openable document that
        // qpdf reads as one page count and writes as another.
        //
        // `retryable: true` even so. It does not mean "retrying would work" -- it means no
        // deliberate gesture is needed to get the page working again. `false` renders the
        // Start again button, which clears the CIRCUIT BREAKER (ADR 0015 §3), and the breaker
        // has not latched here.
        title: "Not Only PDF checked the compressed document and would not hand it over.",
        next: "Your file has not been changed and nothing was sent anywhere. Some of its pages did not survive the write, so Not Only PDF refused the result rather than give you a document quietly short of a page. A copy of the file saved again from the program that made it usually works.",
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
        next: "Your file is fine and nothing was sent anywhere. Try again.",
        retryable: true,
      };
  }
}
