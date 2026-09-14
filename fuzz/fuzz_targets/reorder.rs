//! Fuzz the reordering path — the page-handle permutation, and qpdf's page machinery.
//!
//! ```text
//! # SEED IT FIRST. See "the deepest assertion needs a real PDF" below -- without seeds a
//! # 60-second run never reaches it, which was measured rather than assumed.
//! for f in ../tests/conformance/fixtures/*.pdf; do
//!   { printf '\x05\x05'; cat "$f"; } > "corpus/reorder/seed-$(basename "$f")"
//! done
//!
//! # from fuzz/ -- the RUSTFLAGS override selects the INSTRUMENTED qpdf archive, so
//! # libFuzzer sees inside qpdf's own parser and writer rather than only our wrapper.
//! RUSTFLAGS="-L native=$PWD/../engines/vendor/native-$(uname -m)/lib/fuzz" \
//!   LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run reorder -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # Seed it, and know what it still cannot see
//!
//! An unseeded run reaches the refusal paths and very little else: libFuzzer does not invent
//! a valid multi-page PDF from scratch, so the corpus has to start with real ones. Measured —
//! with `permute` deliberately made to `return Ok(())`, a 60-second unseeded run found nothing
//! in 577,209 executions, and the same build given one conformance fixture failed on the first
//! execution.
//!
//! **It still cannot assert that the pages moved**, and the reason is recorded at the
//! assertion site below rather than here in the summary: it is a fact about PDFs, not about
//! this target, and it caught a wrong assertion before that assertion was pushed.
//!
//! # What this reaches that no other target does
//!
//! - **`qpdf_remove_page` and `qpdf_add_page_at`, on an attacker-built page tree.** No other
//!   target calls either. Both route through `Pages::erase`/`findPage`, which **flattens the
//!   tree** and pushes inherited attributes down first (ADR 0021) — a rewrite of the document's
//!   structure, driven by structure the input chose.
//! - **A handle held across a mutation.** `permute` takes every page's handle up front and
//!   uses them after pages have been removed and re-inserted. That is legal because
//!   `Pages::erase` does `kids.eraseItem(pos)` and leaves the object alone — read from
//!   `QPDF_pages.cc`, and this is where the reading meets real documents. Under ASan, a handle
//!   that stopped being valid shows up here rather than as a wrong answer in production.
//! - **A `/Kids` graph that is not a tree.** A `/Parent` cycle, a page that is its own
//!   ancestor, a branch listed twice — all are two-line edits to a PDF, and all of them reach
//!   qpdf's flattening code through this target.
//!
//! # The order comes from the input, and is usually a real permutation
//!
//! An order that is not a permutation is refused by `Permutation::of` before an engine is
//! touched, so a target that derived its numbers from raw bytes would spend its whole budget
//! on that refusal — which is precisely the bug `split`'s first target had, where every
//! candidate PDF starts `%PDF` and every derived cut was therefore huge.
//!
//! So the first byte chooses a **shuffle seed** and the order is built as a genuine
//! permutation of whatever page count the document turns out to have, with one case in eight
//! left deliberately malformed to keep the refusal path live.
//!
//! Errors are the expected outcome. A panic, a hang, an abort, or a limit escape is a bug —
//! with `max_duration_ms` excepted and said plainly: the clock is a `ManualClock` that never
//! advances, so no input can reach a deadline. A clock that moved on its own would turn every
//! slow input into a `LimitExceeded` and hide whatever that input was really doing. A hang is
//! caught by libFuzzer's `-timeout`; the deadline itself is tested with a stepping clock in
//! `burrow-engines`.

#![no_main]

use std::sync::{Arc, Once};

use burrow_engines::OpenOptions;
use burrow_engines::PageReorderer;
use burrow_engines::qpdf::Qpdf;
use burrow_ops::reorder;
use burrow_types::{Clock, Error, Limits, ManualClock};
use libfuzzer_sys::fuzz_target;

/// Tight limits, so a limit escape is a wrong answer rather than an uninteresting crash.
fn limits() -> Limits {
    Limits::with(|l| {
        l.max_input_bytes = 8 * 1024 * 1024;
        l.max_memory_bytes = 256 * 1024 * 1024;
        l.max_pages = 512;
    })
}

/// Ceilings loose enough to count any document, for the comparisons below.
///
/// Re-opening under `limits()` made `rotate`'s page-count assertion unfalsifiable: a document
/// over `max_pages` failed that open, so the count was `None` and the assertion never ran.
/// Found by code review there; not repeated here.
fn counting() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::with(|l| {
            l.max_input_bytes = u64::MAX;
            l.max_memory_bytes = u64::MAX;
            l.max_pages = u64::MAX;
        }),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

fn page_count(engine: &Qpdf, bytes: &[u8]) -> Option<u64> {
    PageReorderer::open(engine, bytes.to_vec().into_boxed_slice(), &counting())
        .ok()
        .and_then(|source| engine.pages(&source).ok())
}

fn fuzz_mode() {
    static ONCE: Once = Once::new();
    // Upstream's fuzz limits: without them a fuzzer spends its time rediscovering that
    // large files are slow. See `qpdf_check.rs` for the full argument.
    ONCE.call_once(burrow_engines::qpdf::enable_fuzz_mode);
}

/// A permutation of `1..=pages`, shuffled by a cheap deterministic step driven by `seed`.
///
/// Not `rand`: a fuzz target must be a pure function of its input, and a dependency whose
/// version could change what a corpus entry means would make a reproduction unreproducible.
fn shuffled(pages: u64, seed: u64) -> Vec<u64> {
    let mut order: Vec<u64> = (1..=pages).collect();
    if order.len() < 2 {
        return order;
    }
    // A Fisher-Yates pass over a xorshift stream. Every output is a permutation by
    // construction, which is the property this function exists to guarantee -- an order that
    // is merely *probably* a permutation would send most inputs to the same refusal.
    let mut state = seed | 1;
    for i in (1..order.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = usize::try_from(state % (u64::try_from(i).unwrap_or(u64::MAX) + 1)).unwrap_or(0);
        order.swap(i, j);
    }
    order
}

fuzz_target!(|data: &[u8]| {
    fuzz_mode();

    if data.len() < 3 {
        return;
    }

    let seed = u64::from(data[0]) | (u64::from(data[1]) << 8);
    // One case in eight is deliberately NOT a permutation, so the refusal path stays live
    // without swallowing the budget.
    let wanted_malformed = data[1] & 0x07 == 0;
    let document = &data[2..];
    if document.is_empty() {
        return;
    }

    let engine = Qpdf::new();
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(0));
    let options = OpenOptions::new(limits(), clock);

    // THE ORDER IS BUILT AGAINST THE DOCUMENT'S OWN PAGE COUNT, read before the operation.
    // An order built against a guess would be the wrong length for almost every input and
    // every case would end at the same refusal.
    let before = page_count(&engine, document);
    // WHETHER IT IS ACTUALLY MALFORMED, not whether one was asked for. A one-page document has
    // exactly one order and the corruption below is a no-op on it -- so `wanted_malformed`
    // would have made the assertion at the bottom fire on a correct refusal-free run. The two
    // are separate values for that reason.
    let mut malformed = false;
    let order = match before {
        Some(pages) if pages > 0 && pages < 4096 => {
            let mut order = shuffled(pages, seed);
            if wanted_malformed && order.len() >= 2 {
                // Duplicate one entry: the right length, and not a permutation. The failure
                // the `Permutation` type exists to make unexpressible.
                if let Some(first) = order.first().copied() {
                    let last = order.len() - 1;
                    order[last] = first;
                    malformed = true;
                }
            }
            order
        }
        // Unreadable, empty, or implausibly large: send something anyway, so the refusal
        // paths above the engine are exercised on inputs that never open.
        _ => vec![u64::from(data[0]) + 1],
    };

    match reorder(
        &engine,
        document.to_vec().into_boxed_slice(),
        &order,
        &options,
    ) {
        Ok(output) => {
            assert!(
                !output.is_empty(),
                "reorder produced an empty document, which is not a document"
            );

            // THE INVARIANT: a permutation loses no page and adds none. Asserted against the
            // INPUT's count rather than against a ceiling, which would be a tautology for
            // every input libFuzzer can produce.
            let after = page_count(&engine, &output);
            if let (Some(before), Some(after)) = (before, after) {
                // AGAINST A PLAIN WRITE OF THE SAME DOCUMENT, not against the input's count.
                //
                // `after == before` is the assertion anyone would write first, and it is
                // WRONG -- it fails on a correct implementation. A seeded run found a document
                // damaged enough that one page reference does not resolve and intact enough to
                // open: qpdf reports 5 pages, and writing it out yields 4. That happens with
                // no reorder at all. `rotate` does it too, on the same bytes, and `rotate` has
                // shipped. It is a real defect and it is not this operation's -- see the
                // `pdf_with_unresolvable_page` fixture and the issue it names.
                //
                // Attributing it to reorder would have been a false positive; ignoring page
                // count altogether would have given up the invariant. So the IDENTITY
                // permutation, which short-circuits to a plain write, is measured first and
                // used to decide whether this document is one the invariant can be asked of.
                let identity: Vec<u64> = (1..=before).collect();
                let baseline = reorder(
                    &engine,
                    document.to_vec().into_boxed_slice(),
                    &identity,
                    &OpenOptions::new(
                        limits(),
                        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
                    ),
                )
                .ok()
                .and_then(|plain| page_count(&engine, &plain));
                // AND ONLY WHERE THE DOCUMENT IS STABLE UNDER A PLAIN WRITE. Even the
                // baseline is not a fixed point for a damaged document -- a second seeded case
                // went the other way, 6 pages out of the permutation against 3 out of the
                // plain write, because flattening resolves a duplicated page reference by
                // COPYING while a plain write collapses it. The count moves in both
                // directions, and neither movement is the permutation's doing.
                //
                // So the invariant asserted is the one that is actually true: *when writing a
                // document does not by itself change its page count, permuting it must not
                // change it either.* Degenerate inputs are excluded by measurement rather than
                // by a guess about what they look like.
                if baseline == Some(before) {
                    assert_eq!(
                        after, before,
                        "the document survives a plain write with its page count intact, so a \
                         permutation of it must too"
                    );
                }
                assert!(
                    before <= limits().max_pages,
                    "a document over the page ceiling was reordered"
                );
                assert_eq!(
                    u64::try_from(order.len()).unwrap_or(u64::MAX),
                    before,
                    "an order of the wrong length was carried out rather than refused"
                );
                assert!(
                    !malformed,
                    "an order naming a page twice was carried out rather than refused"
                );

                // WHAT THIS TARGET CANNOT ASSERT: that the pages actually moved.
                //
                // Every assertion above is satisfied by a `permute` that did nothing at all --
                // which is exactly the fail-open shape security review found in
                // `ObjectHandle::object`, where `(0, 0) == (0, 0)` makes every page read as
                // "already in place". So an assertion was written for it, comparing a real
                // permutation's output against the same document reordered by the identity.
                //
                // It is not here, because it was WRONG, and the way it was wrong is worth
                // recording. Seeded with `tests/conformance/fixtures/pages-10.pdf`, it failed
                // on the first execution against a correct implementation: that fixture's ten
                // pages are identical objects, and qpdf's writer renumbers objects in
                // traversal order -- so a permutation of interchangeable pages serialises to
                // byte-identical output. The document really was reordered and really did come
                // out the same.
                //
                // That is `add-operation` §2c one layer further out: a fixture whose pages are
                // indistinguishable cannot fail an order test, and here it made the ASSERTION
                // wrong rather than merely vacuous. A fuzzer cannot fix it, because it does not
                // choose its documents -- distinguishability is a property of the input.
                //
                // So the order invariant is measured where a fixture can be chosen:
                // `burrow-ops`' `any_permutation_is_carried_out_exactly` and
                // `no_page_is_lost_added_or_duplicated` read the page order back out of the
                // `/MediaBox` widths, and `burrow-engines`' `reorder_tests` do the same per
                // permutation. What this target measures is the parser, the refusals, and the
                // handle churn under ASan.
            }
        }
        // Every typed error is an acceptable outcome. `Internal` is the one that is not: it
        // means an invariant inside burrow did not hold, which is a finding rather than a
        // refusal.
        Err(Error::Internal(message)) => {
            panic!("reorder reported an internal error: {message}");
        }
        Err(_) => {}
    }
});
