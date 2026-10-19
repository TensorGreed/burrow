/**
 * What `/redact-pdf` must say, and what it must never say (#136, slice S1).
 *
 * # Why the sentences live here rather than only in the page
 *
 * ADR 0029 §7 fixes the copy the redaction page carries: what is removed, what is not and why,
 * the alternative-text cost, the two font disclosures, the refusal, and the one sentence that
 * stops verification reading as a promise -- *"it verifies that it did what it said, not that
 * your secret is gone"*. Each of those is a limit a person looking at a redacted page cannot
 * see for themselves, so a page that silently lost one would be a page that overclaims, and
 * nothing would fail: prose is not type-checked.
 *
 * So, as `components/strip-copy.ts` does for the strip, the page takes these sentences from
 * here and `redact-copy.test.ts` asserts, over the BUILT harness HTML, that every one is
 * present, that "burrow" is absent from the visible prose, and that none of
 * `FORBIDDEN_CLAIMS` matches it.
 *
 * # Wording
 *
 * The §7 marks (`SECTION_7_MARKS`) are verbatim from ADR 0029 §7, which says "Not Only PDF" and
 * never "burrow" because it is written the way it will be served; `redact-copy.test.ts` reads §7
 * out of the ADR and checks each one is in it, so the module cannot drift from the decision. Four
 * marks are not in §7's quoted block and were written for this page in its voice
 * (`PAGE_MARKS`): the original file is unchanged, and the scanned-page paragraph's three
 * sentences, which ADR 0029's *Image re-encoding* section requires to say why in a way that does
 * not read as permanent and does not imply OCR -- the block is writing an edited picture back,
 * not recognising text in it.
 *
 * # The page is held
 *
 * `/redact-pdf` is injected by `astro.config.mjs` only under `BURROW_HARNESS=1`;
 * `production-build.test.ts` asserts it is absent from a production build and present in the
 * harness one. This module ships nothing on its own: nothing in `src/pages/` imports it.
 */

/** §7, what is removed. */
export const REMOVED_MARK =
  "The text you selected is removed from the page, from the file, and from the font's record of what it said.";

/** §7, the three things outside a page that are not changed. */
export const NOT_CHANGED_MARK =
  "Three things are outside a page and are not changed: the document's title and keywords, any files attached to it, and the document's own XMP metadata.";

/** §7, why they are not changed -- without it "the title is not changed" reads as a bug. */
export const NOT_CHANGED_WHY_MARK =
  "Those describe the whole document rather than any page, and Not Only PDF has no way to reach them";

/** §7, the instruction: somewhere to look, rather than "metadata may remain". */
export const WHERE_TO_LOOK_MARK = "Check File → Properties, and check the attachments panel.";

/** §7, the alternative-text cost, which falls on a different person. */
export const ALT_TEXT_MARK =
  "If the passage you redacted from had alternative text, all of it is gone — not just the part you removed.";

/** §7, that the cost is invisible on the page. */
export const ALT_TEXT_UNSEEN_MARK = "Nothing on the page shows this";

/** §7, the first font disclosure: an unshared font keeps the character set. */
export const FONT_SET_MARK =
  "A font that was used only for the text you removed still says which letters that text used.";

/** §7, the second font disclosure: a shared font keeps widths and the code-to-letter map. */
export const FONT_SHARED_MARK =
  "And a font other pages also use keeps more than that: the widths of the removed characters, and the mapping from their codes to the letters they stood for.";

/** §7, that it refuses rather than doing its best. */
export const REFUSES_MARK =
  "Not Only PDF refuses documents it cannot redact safely rather than doing its best, and says which shape it found.";

/**
 * §7, the sentence that stops verification reading as a promise.
 *
 * It is the one place the page may say "your secret is gone", and only negated: the
 * forbidden-claims check removes this exact sentence before it looks for that phrase.
 */
export const VERIFIES_MARK = "it verifies that it did what it said, not that your secret is gone";

/** §7, what verification cannot see, so the sentence above has its reason beside it. */
export const VERIFIES_LIMIT_MARK =
  "it cannot read a secret drawn as a picture or as shapes, and it cannot tell whether a font is telling the truth about what its glyphs say";

/** Written for this page: the original file is not touched, and still holds what was removed. */
export const ORIGINAL_MARK = "The file you chose is not changed.";

/**
 * Written for this page: the scanned-document refusal, said so it does not read as permanent.
 *
 * ADR 0029 (*Image re-encoding*): redaction is region-based, so no recognition is involved;
 * what blocks a scan is writing the edited picture back. "Yet" is the load-bearing word.
 */
export const SCAN_MARK = "Not Only PDF cannot write an edited picture back into the file yet";

/**
 * Written for this page: the refusal a person acts on. `[image-in-region]` refuses any
 * intersection, clipping ignored, and a refusal returns no file at all -- so "touches any part"
 * and "the document", not "covers" and "the page".
 */
export const SCAN_REFUSED_MARK =
  "If the area you select touches any part of a picture, Not Only PDF refuses the document.";

/** Written for this page: that the scan refusal is a step still to take, not a policy. */
export const SCAN_NOT_A_LINE_MARK =
  "That is a step this tool has still to take, not a line it has drawn";

/** The marks quoted verbatim from ADR 0029 §7; the test checks each against the ADR. */
export const SECTION_7_MARKS = [
  REMOVED_MARK,
  NOT_CHANGED_MARK,
  NOT_CHANGED_WHY_MARK,
  WHERE_TO_LOOK_MARK,
  ALT_TEXT_MARK,
  ALT_TEXT_UNSEEN_MARK,
  FONT_SET_MARK,
  FONT_SHARED_MARK,
  REFUSES_MARK,
  VERIFIES_MARK,
  VERIFIES_LIMIT_MARK,
] as const;

/** The marks written for this page, which §7 does not supply. */
export const PAGE_MARKS = [
  ORIGINAL_MARK,
  SCAN_MARK,
  SCAN_REFUSED_MARK,
  SCAN_NOT_A_LINE_MARK,
] as const;

/** Every mark the built page must carry. The count is the measurement. */
export const REDACT_MARKS = [...SECTION_7_MARKS, ...PAGE_MARKS] as const;

/**
 * Claims the page must never make, each with a fixture it must catch and a near miss it must
 * not, so the list is a measurement rather than a hope (`redact-copy.test.ts` runs both on
 * every run).
 *
 * ADR 0029 §6 says what verification does not establish: that the secret is gone, that the
 * region is blank, that pictures and shapes were redacted, that metadata was touched. A page
 * saying any of those is overclaiming in the one place an overclaim leaks a secret.
 *
 * WHAT THIS LIST IS NOT: a judge of meaning. Each rule catches the common wordings of one claim,
 * and its fixtures say which; a sentence that makes the claim in words none of them anticipated
 * passes. It stops the easy overclaim from landing unnoticed in an edit, and it does not stand in
 * for reading the page against §6 and §7 when the copy changes.
 */
export const FORBIDDEN_CLAIMS: readonly {
  name: string;
  pattern: RegExp;
  caught: string;
  nearMiss: string;
}[] = [
  {
    name: "secure",
    pattern: /\bsecure(ly|d)?\b/i,
    caught: "Redact your PDF securely.",
    nearMiss: "It refuses documents it cannot redact safely.",
  },
  {
    name: "permanent",
    pattern: /\bpermanent(ly)?\b/i,
    caught: "The text is permanently removed.",
    nearMiss: "The text is removed from the page.",
  },
  {
    name: "guarantee",
    pattern: /\bguarantee/i,
    caught: "We guarantee nothing is left.",
    nearMiss: "It says which shape it found.",
  },
  {
    name: "compliant",
    pattern: /\bcomplian(t|ce)\b/i,
    caught: "HIPAA-compliant redaction.",
    nearMiss: "It complies with what you selected.",
  },
  {
    name: "a named regulation",
    pattern: /\b(HIPAA|GDPR|FOIA|PCI)\b/,
    caught: "Ready for GDPR requests.",
    nearMiss: "Ready when you are.",
  },
  {
    name: "irreversible",
    pattern:
      /\b(irreversibl[ey]|unrecoverable|irrecoverabl[ey]|can(not|'t| never) be (recovered|restored|undone))\b|\bnothing\b[^.]*\bcan be (recovered|restored)\b/i,
    caught: "The removal is irreversible.",
    nearMiss: "The removal is verified.",
  },
  {
    name: "gone for good",
    pattern: /\b(for good|forever)\b/i,
    caught: "Your secret is gone for good.",
    nearMiss: "The alternative text is gone.",
  },
  {
    name: "the secret is gone",
    pattern: /\b(secret|text|data|information)s? (is|are) (now )?(gone|destroyed|erased)\b/i,
    caught: "Once it finishes, your secret is gone.",
    nearMiss: "If it had alternative text, all of it is gone.",
  },
  {
    name: "completely",
    pattern: /\b(completely|fully|totally|100 ?%)\b/i,
    caught: "Text is completely removed.",
    nearMiss: "Text is removed.",
  },
  {
    name: "the area is blank",
    pattern: /\bblank\b/i,
    caught: "The area is left blank.",
    nearMiss: "The area you selected.",
  },
  {
    name: "pictures or shapes are redacted",
    pattern:
      /\b(images?|pictures?|photos?|shapes?|graphics|drawings?)(\s+[\w']+){0,4}?\s+(are|is|get|gets) (also )?(redacted|removed|blacked out)\b|\bredacts? (images|pictures|photos|shapes|graphics)\b/i,
    caught: "Pictures are redacted too.",
    nearMiss: "It cannot read a secret drawn as a picture.",
  },
  {
    name: "metadata is scrubbed",
    pattern:
      /\b(scrub|sanitis|sanitiz)|\bmetadata (is|are|gets) (also )?(removed|stripped|cleaned|cleared|wiped)\b|\b(removes?|strips?|cleans?|clears?|wipes?) (all |the |its )*metadata\b/i,
    caught: "It also strips the metadata.",
    nearMiss: "The document's own XMP metadata is not changed.",
  },
  {
    name: "safe to share",
    pattern: /\bsafe to (share|send|publish|post)\b/i,
    caught: "Your file is now safe to share.",
    nearMiss: "Share it once you have checked the properties.",
  },
  {
    // D9 (owner, 2026-10-19): a re-fed result is an ordinary input, and no text may imply that the
    // redaction it came from was verified. A run checks its own output (ADR 0022), which says
    // nothing a person may lean on about the file the run started from.
    name: "the earlier redaction was verified",
    pattern:
      /\b(already|previously|earlier|previous|last|first|prior)\b[^.]{0,40}?\b(verified|checked|confirmed|validated)\b|\b(verified|checked|confirmed|validated) (result|redaction|file|copy|output|areas?)\b/i,
    caught: "The earlier redaction was verified, so only the new area is checked.",
    nearMiss: "Download the last result, with only the earlier areas removed.",
  },
  {
    name: "text recognition",
    pattern: /\b(OCR|recogni[sz]e[sd]?|recogni[sz]ing|recognition)\b/i,
    caught: "Scanned pages need OCR.",
    nearMiss: "Scanned pages are refused for now.",
  },
] as const;
