# Decisions already made

One line per decision, the issue or ADR where it was made, and what it does **not** cover. This
is the index of answers so a question already settled is applied, not re-litigated — see the
standing instruction in [`CLAUDE.md`](../CLAUDE.md#applying-a-recorded-decision). Each rule was
decided in a thread, not here; this page records them, it does not invent them. Add a rule when
one is decided in an issue, with the same three parts.

## Rules

1. **Refuse rather than model PDFium.** Where a renderer's reading of a structure diverges from
   qpdf's, do not carry a second geometry or transform in step with PDFium (that is #111-class
   residue); act on declared structure and refuse what the two read differently.
   *Decided:* #228 (inline-image `/L` vs the dictionary), with the divergence family #152, #224,
   #227, #229. *Does not cover:* inputs where the two agree — extent is computed there, not refused.

2. **Measure before proposing a rule.** Run the census (the real set plus the fixtures), register
   the bar first, hold to 1% of real documents refused. Over the bar: bring the number, do not
   adjust the rule. *Decided:* #227 (the census), #242 (measuring a rule's false-refusal rate on
   real documents). *Does not cover:* fixtures — deliberately malformed, not counted against the
   1%-real bar.

3. **Fail closed.** A removal nothing observed is not a measured removal; a verify must witness the
   removal, not infer it from a blind instrument. *Decided:* ADR 0029 §8. *Does not cover:* channels no instrument observed — those
   are unmeasured, not clean.

4. **Prefer the wider rule; record its over-refusal.** A rule with a known hole is not a narrower
   rule. Take the wider rule and record its over-refusal rate rather than narrowing it to dodge a
   false positive. *Decided:* #229, with #242 measuring the rate. *Does not cover:* a narrowing
   chosen only to avoid recording the rate.

5. **No new hostile-byte parser, and no new dependency, for an unmeasured shape.** If no measured
   document exhibits the shape, file it for later rather than growing the attack surface or the
   dependency set. *Decided:* ADR 0013's 2026-10-05 amendment (the recovery-on detector measured
   and rejected) and #61. *Does not cover:* a shape a measured real document actually uses — then
   it is in scope.

6. **Size budget: re-record growth, ask to move the number.** Growth within a budget is
   re-recorded with a `why`; moving a budget number is an ask; the 25%-over-measurement policy
   sets a new figure. *Decided:* #202. *Does not cover:*
   moving the number without an ask — that is the ask.

7. **Every rule ships a fixture, its near-miss twin, and a planted mutation.** The mutation is
   asserted-applied and rebuilt; a fixture that pins a refusal's shape must also draw into the
   redacted region. *Decided:* the fixture PR #262 (with #263) and #258. *Does not cover:* a
   fixture that pins a refusal but never draws into the region — it cannot fail an order or
   coverage test.

8. **Checks compare an explicit answer; never infer "not applicable" from a missing field.** A
   gate reports what it examined and gates on the expected count where it is knowable, rather than
   reading a missing field as a pass. *Decided:* cross-cutting CI-gate hardening — see
   `CLAUDE.md` "every check reports what it examined" (the "4 of 15 reads as success" lesson), the
   closest issue being #149. *Does not cover:* a count that is genuinely not knowable — then name
   what was examined.

9. **qpdf object-resolving calls are parsing, under the trap — never trapped functions.** A
   binding that resolves objects is a parsing call made inside the exception trap; the prescan
   reads declarations only and never calls one. *Decided:* ADR 0013 (handle-identity and prescan),
   #224. *Does not cover:* the prescan — it must stay declaration-only.

10. **The spec-review question for every redaction or split change:** what does this ACCEPT that
    ADR 0029 §3 / ADR 0019 §2b says to refuse, and what does it REFUSE that is a legitimate
    document? *Decided:* ADR 0029 §3, ADR 0019 §2b. *Does not cover:* changes that touch neither
    redaction nor split geometry.

11. **A run-level CI conclusion other than `success` is a stop — even when every job, step and
    check-run is green.** The merge gate is GitHub CI green on the head, and "green" means the
    *run* concluded `success`, not merely that no job failed. A run that concludes
    `failure`/`cancelled`/`timed_out` while its jobs and check-suite are all `success` is
    unexplained until shown otherwise: investigate attempts, annotations, the check-suite, and any
    skipped/cancelled job; if no real failure is found, re-run CI on the head (or confirm the
    `push`-event run on the merged SHA) and require a `success` run before merge. Never merge on
    the reasoning "every job looks green." *Decided:* #275, 2026-10-07 — a `pull_request` run
    concluded `failure` with all 10 jobs `success`, attempt 1, only `notice`/`warning` annotations;
    the `push` run on the identical merged tree concluded `success`. *Does not cover:* a run whose
    non-success is a real job/step failure — that is an ordinary red, fix it.

12. **Never hand-merge `tests/redaction/outcomes.tsv`; regenerate it from the merged tree.** It is
    the golden the native and web differentials replay, and the only file #125 and #137 both
    touch. On every rebase onto `main`, regenerate it from the merged tree
    (`BURROW_BLESS_REDACTION_OUTCOMES=1` over the native redaction outcomes) and commit the
    regenerated version rather than resolving a conflict by hand. **If the regenerated file differs
    from what your own change predicts, stop and report** — a surprise there means the merged base
    changed an outcome your change did not account for. *Decided:* owner, 2026-10-07, for the
    parallel #125 (core) and #137 (web/wasm) sessions. *Does not cover:* a change that does not
    alter any fixture outcome — then the regenerated file is identical and there is nothing to
    reconcile.

13. **No shipped build can read qpdf's error text; fuzz builds may, bounded by a check.** Rule 3 of
    `core/burrow-engines/src/codes/qpdf.rs` -- burrow does not declare the functions that return
    qpdf's error text, because that text quotes the file, and "a function that cannot be called
    cannot leak" -- holds for every shipped build. The one exception is
    `qpdf_get_error_message_detail`, declared behind `burrow-engines`' `fuzzing` feature and read by
    one function that compares it to fixed constants and returns a fixed burrow message, so the
    nightly can tell #285's qpdf invariant failure from burrow's own handle misuse. The exception is
    bounded by `tools/check-no-error-text-in-deploy.sh`, which fails a deploy build where any of the
    three wasm feature sets, or `burrow-ffi` at its default features on aarch64 Android or iOS
    (not the emulator or simulator targets, named in the check), turns `fuzzing` on, or where any
    file under `dist/` carries an error-text function name. On wasm the declaration cannot compile
    at all (`qpdf` is native Linux only), so there the real bound is qpdf.wasm's export allowlist
    and the check is defence in depth; the `burrow-ffi` half is what bounds a future native mobile
    build. *Decided:* owner, 2026-10-08, #285. *Does not cover:* reading any other qpdf
    text, or reading this one in a shipped build -- each is a new decision, not an extension.

14. **A standard-14 width is accepted only where the AFM and PDFium's bundled face agree; the
    system-font residual is accepted and recorded.** For a font with no `/Widths`, burrow places a
    glyph by NAME from `core/burrow-engines/src/pdfsyntax/standard14_table.rs`, generated (never
    edited) from Adobe's Core 14 AFMs (ADR 0030) intersected with the pinned PDFium's measured
    advances, per style; `tools/check-standard14-table.sh` regenerates and diffs it, and a face gate
    requires PDFium to have loaded exactly the measured face. **The residual, stated:** the accepted
    widths are exact for viewers that use the AFM metrics, and for PDFium drawing from its bundled
    faces. A viewer that substitutes a system font for a standard-14 name -- desktop Chrome's PDFium
    among them, when a matching face is installed -- may lay text out differently. This is the same
    residual the hand-transcribed table already had, not a new one. *Decided:* owner, 2026-10-09,
    #290. *Does not cover:* native mobile builds, which will see system fonts: revisit
    `FPDF_SetSystemFontInfo` (making PDFium see none) at M3/M4 as its own decision; and any other
    font class.

## Spec review first (the practice that uses rule 10)

For every new rule from here: **before writing code**, give the `security-reviewer` the proposed
rule, the ADR 0029 §3 / ADR 0019 §2b table, and the census plan, and ask for (a) the inputs the
rule would accept that the table says to refuse, and (b) the fixture list it would demand. Build
from that list. The post-code reviews still run; the aim is that round 2 finds nothing a round 0
could have. (Recorded as a working agreement in [`CLAUDE.md`](../CLAUDE.md#spec-review-first).)
