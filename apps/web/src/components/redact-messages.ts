// What the redaction page says when it does not hand a document back (#136, S3 and S7).
//
// # Categories, not rules
//
// The core refuses for about 140 reasons, each with a stable rule id in brackets inside its
// message. Per-rule copy is post-launch (owner, 2026-10-14, S7). For launch, each rule belongs to
// one of a handful of plain-language CATEGORIES, and the page says the category's sentence and
// what the visitor can do, with the rule id beside it so a report can name the exact cause.
//
// # The rule id is matched, never shown as read
//
// ADR 0009 keeps engine and module prose off the page: a message can carry text derived from the
// input. The page therefore does not display `message`. It looks for a bracketed token, and shows
// it only if that token is a key of `RULES` below -- so what reaches the DOM is always one of this
// file's own strings, whatever the message held. An unknown token is not shown at all.
//
// # Every rule the core raises has a category, checked against the source
//
// `redact-messages.test.ts` derives the rule vocabulary from the Rust sources and the committed
// outcomes golden on every run, and fails naming any rule this table does not place. A rule added
// to the core without a category here turns that test red rather than reaching a visitor as the
// fallback.
//
// # What the copy may say
//
// ADR 0029 §7 and §6. It never says the secret is gone, permanent, secure or compliant; never that
// an area is blank; never that pictures or shapes are redacted. The test holds every sentence here
// to `FORBIDDEN_CLAIMS`, the same list the page's own copy is held to, and to "Not Only PDF", never
// "burrow".

/** The fields of a failed reply this module reads. */
export interface RedactFailure {
  /** The typed variant's name, e.g. `Unsupported`. */
  kind: string;
  /** Engine-written; read only for a bracketed rule id, never displayed. */
  message?: string;
  /** The ceiling that was hit, for `LimitExceeded`. */
  limit?: string;
  /** What was asked for, and what was allowed, for `LimitExceeded`. Strings: these are `u64`. */
  requested?: string;
  allowed?: string;
}

export interface RedactMessage {
  /** One line, said plainly. */
  title: string;
  /** What the visitor can do next. Empty when there is genuinely nothing to do. */
  next: string;
  /** The rule id, when the reply carried one this page knows; otherwise "". */
  rule: string;
  /** Whether trying again on this page can help without a deliberate reset. */
  retryable: boolean;
}

/** The categories a refusal is grouped into (S7: about 6-10). */
export type Category =
  | "pictures"
  | "annotations"
  | "layers"
  | "page-shape"
  | "shared"
  | "fonts"
  | "text-structure"
  | "file-structure"
  | "too-complex";

/** Each category's sentence and its one next step. */
export const CATEGORIES: Readonly<Record<Category, { title: string; next: string }>> = {
  pictures: {
    title:
      "The area you chose touches a picture or a drawn shape, and Not Only PDF removes only text: it cannot write an edited picture back into the file yet, so a scanned page cannot be redacted here yet either.",
    next: "Choose an area that holds only text. If the page is a scan, the section on scanned documents below says what that means for now.",
  },
  annotations: {
    title:
      "The area you chose has a comment, a form field or another annotation over it, and Not Only PDF will not decide what to do with it for you.",
    next: "Flatten or remove the annotation in the program you use to edit the PDF, then try again.",
  },
  layers: {
    title:
      "This document has layers that can be switched on and off, and Not Only PDF cannot tell what a hidden layer would show.",
    next: "Flatten the layers in the program that made the document, or print it to a new PDF, then try again.",
  },
  "page-shape": {
    title: "Not Only PDF could not place that area on this page as it is shown.",
    next: "Check the page number and the area against the page's size, or save the document again from the program that made it.",
  },
  shared: {
    title:
      "The text in the area you chose is also drawn on another page, so removing it here would change that page too.",
    next: "Use Split PDF to take this page out on its own, and redact that file.",
  },
  fonts: {
    title: "A font in this document is described in a way Not Only PDF cannot read reliably.",
    next: "Save the document again from the program that made it, or print it to a new PDF, then try again.",
  },
  "text-structure": {
    title: "The text on this page is written in a way Not Only PDF cannot follow exactly.",
    next: "Save the document again from the program that made it, or print it to a new PDF, then try again.",
  },
  "file-structure": {
    title:
      "Parts of this file can be read in more than one way, and Not Only PDF will not guess which one you see.",
    next: "Save the document again from the program that made it, then try again.",
  },
  "too-complex": {
    title: "This page is more complicated than Not Only PDF goes through.",
    next: "Try a smaller area, or a simpler copy of the page.",
  },
};

/**
 * Every rule the core can refuse a redaction with, and its category.
 *
 * Grouped by category below for reading; the test checks it covers the source's vocabulary.
 */
export const RULES: Readonly<Record<string, Category>> = {
  // Pictures and shapes in or across the region.
  "image-in-region": "pictures",
  "vector-in-region": "pictures",
  "shading-in-region": "pictures",
  "pattern-may-draw-text": "pictures",
  "type-three-image-cut": "pictures",
  "type-three-image-outside-its-box": "pictures",
  "type-three-procedure-paints": "pictures",
  "too-much-ink": "pictures",
  "clip-path-in-region": "pictures",

  // Annotations and form fields (D3: refused, never silently kept or removed).
  "acroform-field": "annotations",
  "annotation-appearance-unbounded": "annotations",
  "annotation-dependent-kept": "annotations",
  "annotation-in-region": "annotations",
  "annotation-no-rotate": "annotations",
  "annotation-popup-unreadable": "annotations",
  "annotation-quads-outside-rect": "annotations",
  "annotation-rect": "annotations",
  "annots-too-long": "annotations",

  // Optional content.
  "optional-content": "layers",
  "optional-content-marked": "layers",

  // The page's frame.
  "display-box": "page-shape",
  "no-display-box": "page-shape",
  "media-box-unverified": "page-shape",
  "page-attribute-inherited": "page-shape",
  "page-frame-unreadable": "page-shape",
  "page-out-of-range": "page-shape",
  "region-extent": "page-shape",
  "rotate-not-whole": "page-shape",
  "user-unit": "page-shape",
  "user-unit-not-one": "page-shape",
  rotate: "page-shape",

  // Content another page draws too.
  "shared-contents": "shared",
  "shared-form-would-change-elsewhere": "shared",

  // Fonts.
  "base-encoding-not-a-name": "fonts",
  "direct-font": "fonts",
  "font-selected-in-another-scope": "fonts",
  "descendant-fonts": "fonts",
  "differences-anchor": "fonts",
  "encoding-not-a-dictionary": "fonts",
  "differences-item-unreadable": "fonts",
  "differences-too-long": "fonts",
  "first-char": "fonts",
  "font-flags": "fonts",
  "font-missing": "fonts",
  "mixed-code-widths": "fonts",
  "no-font-selected": "fonts",
  "no-width": "fonts",
  "no-widths": "fonts",
  "simple-font-multi-byte-codes": "fonts",
  "type3-matrix": "fonts",
  "type-three-procedure-matrix": "fonts",
  "type-three-procedure-shows-text": "fonts",
  "type-three-unreadable": "fonts",
  "type-three-width-zero": "fonts",
  "tounicode-inline-image-syntax": "fonts",
  "unreadable-cmap": "fonts",
  "vertical-writing": "fonts",
  "width-out-of-range": "fonts",
  "last-char": "fonts",
  "width-source": "fonts",
  "widths-overlap": "fonts",
  "widths-too-long": "fonts",
  "widths-too-many": "fonts",
  "writing-mode-undetermined": "fonts",
  "zero-bytes-per-code": "fonts",

  // How the page's text is written.
  "adjustment-not-expressible": "text-structure",
  "bt-nested": "text-structure",
  "bt-unterminated": "text-structure",
  "do-in-text-object": "text-structure",
  "do-operand-not-a-name": "text-structure",
  "et-without-bt": "text-structure",
  "ext-gstate-line-unreadable": "text-structure",
  "ext-gstate-sets-font": "text-structure",
  "glyph-from-another-stream": "text-structure",
  "glyph-on-a-non-showing-operation": "text-structure",
  "glyph-without-its-operation": "text-structure",
  "graphics-state-in-text": "text-structure",
  "gs-operand-not-a-name": "text-structure",
  "marked-content-carries-opaque-string": "text-structure",
  "marked-content-named-properties-carry-text": "text-structure",
  "marked-content-properties-unresolved": "text-structure",
  "structure-tree": "text-structure",
  "marked-content-property-list-malformed": "text-structure",
  "marked-content-split-across-elements": "text-structure",
  "non-finite-geometry": "text-structure",
  "numeric-operand-not-a-number": "text-structure",
  "operand-count-mismatch": "text-structure",
  "q-unbalanced": "text-structure",
  "q-without-save": "text-structure",
  "show-operand-not-a-string": "text-structure",
  "string-not-whole-codes": "text-structure",
  "text-outside-text-object": "text-structure",
  "text-state-into-form": "text-structure",
  "tj-array-item-not-showable": "text-structure",
  "tj-operand-not-an-array": "text-structure",
  "unknown-operator": "text-structure",
  "form-matrix": "text-structure",
  "xobject-missing": "text-structure",

  // How the file itself is written.
  "contents-boundary": "file-structure",
  "contents-not-a-stream": "file-structure",
  "contents-offset": "file-structure",
  "contents-unreadable": "file-structure",
  "document-poisoned": "file-structure",
  "embedded-nul": "file-structure",
  "engine-repaired-input": "file-structure",
  "form-unreadable": "file-structure",
  "form-vanished": "file-structure",
  "high-byte-separator": "file-structure",
  "inline-image-bpc-without-cs": "file-structure",
  "inline-image-cs-without-bpc": "file-structure",
  "inline-image-data-holds-ei": "file-structure",
  "inline-image-end-unrecognised": "file-structure",
  "inline-image-escaped-key": "file-structure",
  "inline-image-filtered": "file-structure",
  "inline-image-key-without-value": "file-structure",
  "inline-image-length-disagrees": "file-structure",
  "inline-image-mask-with-cs": "file-structure",
  "inline-image-nested-bi": "file-structure",
  "inline-image-nested-value": "file-structure",
  "inline-image-repeated-key": "file-structure",
  "inline-image-unknown-key": "file-structure",
  "inline-image-unshared-cs": "file-structure",
  "missing-slash": "file-structure",
  "name-read-differently": "file-structure",
  "name-token-too-long": "file-structure",
  "not-a-dictionary-where-one-belongs": "file-structure",
  "null-value": "file-structure",
  "number-unreadable": "file-structure",
  "reference-to-nothing": "file-structure",
  "reference-unreadable": "file-structure",
  "subtype-not-a-name": "file-structure",
  "syntax-read-differently": "file-structure",

  // The second reading (#206): PDFium reads the redacted page again, and a disagreement with
  // burrow's own reading, or no answer at all, refuses (ADR 0029 §6).
  //
  // A VISITOR NEVER SEES THESE CATEGORIES. The second reading's refusals arrive as
  // `OutputRejected`, and `messageFor` answers that kind with its own copy ("checked the result and
  // would not hand it over") whatever the rule, keeping only the rule id; the test holds all nine
  // to that. The categories below are placeholders, there so every id the engine lists has one,
  // until verification gets its own copy. They are not a choice of what to tell anyone, and four
  // would read wrongly if shown: region-too-small, wrong-page, unavailable and nesting carry next
  // steps that do not fit what went wrong.
  "second-reading-glyph-in-region": "text-structure",
  "second-reading-no-text": "text-structure",
  "second-reading-page-size": "page-shape",
  "second-reading-wrong-page": "page-shape",
  "second-reading-region-too-small": "page-shape",
  "second-reading-page-count": "file-structure",
  "second-reading-input-page-count": "file-structure",
  "second-reading-unavailable": "file-structure",
  "second-reading-nesting": "too-complex",

  // Past a ceiling the walk keeps.
  "contents-too-large": "too-complex",
  "contents-too-many": "too-complex",
  "form-cycle": "too-complex",
  "form-depth": "too-complex",
  "form-graph-too-deep": "too-complex",
  "form-graph-too-large": "too-complex",
  "page-tree-depth": "too-complex",
  "properties-too-many": "too-complex",
  "resource-graph-cycle": "too-complex",
  "resource-graph-depth": "too-complex",
  "resource-graph-work": "too-complex",
  "too-many-form-draws": "too-complex",
  "too-many-glyphs": "too-complex",
};

/**
 * The rule id in `message`'s FIRST bracketed token, if this page knows it; otherwise "".
 *
 * The first token only, as Rust's `FrameRule::of` reads it (`redact/frames.rs`): the core writes
 * the rule there, and a later bracket can be anything the message quotes.
 */
export function ruleOf(message: string | undefined): string {
  if (!message) return "";
  // ANCHORED ON THE FIRST `[`, then validated (L1, round 2's A): a first bracket that does not
  // close, or holds anything but one of this page's rules, is no rule -- a later bracket is never
  // tried in its place.
  const open = message.indexOf("[");
  if (open < 0) return "";
  const close = message.indexOf("]", open + 1);
  if (close < 0 || close - open - 1 > 64) return "";
  const id = message.slice(open + 1, close);
  return /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(id) && Object.hasOwn(RULES, id) ? id : "";
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

/** A ceiling, named, as `split-messages.ts` names it. "May", because it is not a promise. */
function limitMessage(failure: RedactFailure): RedactMessage {
  const base = { rule: "", retryable: true };
  switch (failure.limit) {
    case "max_input_bytes": {
      const asked = mb(failure.requested);
      const allowed = mb(failure.allowed);
      return {
        ...base,
        title:
          asked && allowed
            ? `That file is ${asked}, and Not Only PDF stops at ${allowed}.`
            : "That file is larger than Not Only PDF will open.",
        next: "Nothing was read. A smaller file, or one split into parts first, may work.",
      };
    }
    case "max_pages": {
      const asked = count(failure.requested);
      const allowed = count(failure.allowed);
      return {
        ...base,
        title:
          asked && allowed
            ? `That document has ${asked} pages, and Not Only PDF stops at ${allowed}.`
            : "That document has more pages than Not Only PDF will open.",
        next: "It was opened but not changed. Splitting it into smaller documents first may work.",
      };
    }
    case "max_duration_ms":
      return {
        ...base,
        title: "That took longer than Not Only PDF allows and was stopped.",
        next: "Nothing was changed. A smaller area, or a simpler copy of the page, may finish.",
      };
    case "max_memory_bytes":
      return {
        ...base,
        // DETECTED, NOT PREVENTED (ADR 0007): the memory was already spent when it was noticed.
        title: "That document used more memory than Not Only PDF allows, and was stopped.",
        next: "It had already been read by then, so the tab may be slow for a moment. Nothing was changed.",
      };
    default:
      return {
        ...base,
        title: "That document is past one of the limits Not Only PDF works within.",
        next: "Nothing was changed. A smaller document may work.",
      };
  }
}

/** The category's message for `rule`, which must be a key of `RULES`. */
export function messageForRule(rule: string): RedactMessage {
  const category = RULES[rule];
  if (!category) return messageFor({ kind: "Unsupported" });
  const { title, next } = CATEGORIES[category];
  return { title, next, rule, retryable: true };
}

/** What the page says about a failed reply. */
export function messageFor(failure: RedactFailure): RedactMessage {
  const rule = ruleOf(failure.message);
  if (rule && (failure.kind === "Unsupported" || failure.kind === "Malformed")) {
    return messageForRule(rule);
  }
  switch (failure.kind) {
    case "PasswordRequired":
      return {
        title: "That PDF is password-protected.",
        // FOR LAUNCH, A REFUSAL (owner, 2026-10-15): a password field would hold the password in a
        // JavaScript string nothing can zero (#199); it is post-launch.
        next: "Not Only PDF cannot open it yet. Remove the password in the program you read it with, then choose it again.",
        rule: "",
        retryable: true,
      };
    case "LimitExceeded":
      return limitMessage(failure);
    case "OutputRejected":
      return {
        title: "Not Only PDF checked the result and would not hand it over.",
        next: "Your file has not been changed and no new file was made. If it keeps happening on this document, that is worth reporting.",
        rule: ruleOf(failure.message),
        retryable: true,
      };
    case "Malformed":
      return {
        title: "Not Only PDF could not finish reading that document.",
        next: "If it opens elsewhere, the problem is here rather than in your file. Nothing was changed.",
        rule: "",
        retryable: true,
      };
    case "Unsupported":
      return {
        title: "That PDF uses something Not Only PDF does not handle yet.",
        next: "Nothing was changed. Saving it again from the program that made it sometimes helps.",
        rule: "",
        retryable: true,
      };
    case "InvalidArgument":
      return {
        title: "Not Only PDF could not make sense of that request.",
        next: "This is a fault in the page rather than a problem with your file. Reloading may clear it.",
        rule: "",
        retryable: true,
      };
    case "EngineUnavailable":
      return {
        title: "Not Only PDF has stopped after several failures in a row.",
        next: "Nothing is running. Choose Start again when you are ready.",
        rule: "",
        retryable: false,
      };
    default:
      return {
        title: "Something inside Not Only PDF failed.",
        next: "Your file is fine and nothing was sent anywhere. Try again.",
        rule: "",
        retryable: true,
      };
  }
}

/**
 * What a finished run says about the two things §7 reports only when they happened (S3).
 *
 * Counts from the reply, said as sentences: `retainedFonts` is how many fonts were left whole
 * because a page outside the run also uses them, and `droppedCarriedText` how many passages lost
 * their alternative text with the removal.
 */
export function resultNotes(retainedFonts: number, droppedCarriedText: number): string[] {
  const notes: string[] = [];
  if (retainedFonts > 0) {
    const one = retainedFonts === 1;
    notes.push(
      `${one ? "One font" : `${retainedFonts} fonts`} on this page ${one ? "is" : "are"} also used on other pages, so Not Only PDF left ${one ? "it" : "them"} whole: ${one ? "it still holds" : "they still hold"} the widths of the removed characters, and the mapping from their codes to the letters they stood for.`,
    );
  }
  if (droppedCarriedText > 0) {
    const one = droppedCarriedText === 1;
    notes.push(
      `${one ? "One passage" : `${droppedCarriedText} passages`} you removed text from had alternative text, and all of it is gone, not just the part you removed. Nothing on the page shows this, and a screen reader now reads the letters on the page instead.`,
    );
  }
  return notes;
}
