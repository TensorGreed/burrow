//! The web orchestration, driven against a fake bridge on an ordinary host.
//!
//! These are the tests that a `cfg(target_arch = "wasm32")` module could not have had.
//! They assert the things Playwright cannot see from outside a worker — that the buffer is
//! freed on every failure path, that the password copy in the engine heap is wiped, that
//! the page limit is checked before a document escapes, and that the checks happen in the
//! order the native path uses.

use std::sync::Arc;

use burrow_types::{
    Clock, Error, Limits, ManualClock, Password, Permutation, Result, Rotation, Stage,
};

use super::fake::{Call, FakeHeap, FakePdfium, FakeQpdf, PdfiumScript, QpdfScript};
use super::{QpdfBridge, WebPdfium, WebQpdf};
use crate::{
    CheckOptions, DocumentCompressor, DocumentEngine, OpenOptions, PageAssembler, PageExtractor,
    PageReorderer, PageRotator, StructureEngine,
};

/// A stopped clock, so nothing here depends on how busy the machine is.
fn stopped() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(0))
}

/// A PDF the pre-scan and the engine both accept. Content is irrelevant to the fake; what
/// matters is that it is a plausible PDF, so the pre-scan does not reject it first.
fn ordinary_pdf() -> Vec<u8> {
    crate::minimal_pdf::pdf_with_pages(1)
}

fn document_engine(script: PdfiumScript) -> (WebPdfium, Arc<FakeHeap>) {
    let state = FakeHeap::new();
    let bridge = Arc::new(FakePdfium::new(Arc::clone(&state), script));
    (WebPdfium::new(bridge), state)
}

fn structure_engine(script: QpdfScript) -> (WebQpdf, Arc<FakeHeap>) {
    let state = FakeHeap::new();
    let bridge = Arc::new(FakeQpdf::new(Arc::clone(&state), script));
    (WebQpdf::new(bridge), state)
}

#[test]
fn the_engines_name_themselves_distinctly_from_the_native_ones() {
    // The differential harness reports which implementation produced an outcome, so the
    // two must not share a name.
    let (pdfium, _) = document_engine(PdfiumScript::default());
    let (qpdf, _) = structure_engine(QpdfScript::default());
    assert_eq!(pdfium.name(), "pdfium-wasm");
    // Spelled out because `WebQpdf` implements TWO traits that both offer `name`, since
    // M1 PR B2 added `PageAssembler`. The differential harness keys outcomes by engine
    // name, so the two must agree -- `assemble.rs` asserts that directly, and this asserts
    // the value.
    assert_eq!(StructureEngine::name(&qpdf), "qpdf-wasm");
}

#[test]
fn an_ordinary_document_opens_and_reports_its_pages() {
    let (engine, state) = document_engine(PdfiumScript {
        page_count: 7,
        ..PdfiumScript::default()
    });
    let doc = engine
        .open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(Limits::default(), stopped()),
        )
        .expect("an ordinary document should open");
    assert_eq!(doc.pages_at_open(), 7);
    assert_eq!(engine.page_count(&doc).expect("readable"), 7);
    drop(doc);
    state.assert_empty();
}

/// The guard releases the document through the one call that cannot get the order wrong,
/// exactly once.
///
/// PDFium reads from the input buffer for as long as the document is open
/// (`fpdfview.h:451`), so freeing before closing is a use-after-free.
/// [`PdfiumBridge::close_document`] does both in one call, which is why there is no call
/// site at which the wrong order is expressible — and why **this test cannot observe the
/// ordering itself**. What it pins is the half that is Rust's: the guard uses that call, on
/// a normal drop, exactly once. The ordering inside it is the JS bridge's, and is covered
/// by 4a-ii's browser tests.
///
/// Naming it after the ordering would have overclaimed, so it is not.
#[test]
fn dropping_a_document_releases_it_exactly_once_through_the_combined_call() {
    let (engine, state) = document_engine(PdfiumScript::default());
    let doc = engine
        .open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(Limits::default(), stopped()),
        )
        .expect("should open");
    drop(doc);

    let closes: Vec<_> = state
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::CloseDocument(..)))
        .collect();
    assert_eq!(closes.len(), 1, "the document must be closed exactly once");
    state.assert_empty();
}

/// Every failure path after the buffer reaches the engine heap must still free it.
///
/// On the web nothing notices a leak: there is no allocator on the other side of the bridge
/// to complain, so the worker simply grows. This is the test that makes the ownership
/// discipline checkable rather than assumed.
#[test]
fn every_failure_after_the_copy_still_frees_the_engine_buffer() {
    // A failed load, with each of PDFium's error codes.
    for code in [1_u64, 2, 3, 4, 5, 6] {
        let (engine, state) = document_engine(PdfiumScript {
            load_succeeds: false,
            load_error_code: code,
            ..PdfiumScript::default()
        });
        let result = engine.open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(Limits::default(), stopped()),
        );
        assert!(result.is_err(), "code {code} should have failed");
        state.assert_empty();
    }

    // An unreadable page tree, which happens after the document exists.
    let (engine, state) = document_engine(PdfiumScript {
        page_count: -1,
        ..PdfiumScript::default()
    });
    assert!(matches!(
        engine.open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(Limits::default(), stopped())
        ),
        Err(Error::Malformed(_))
    ));
    state.assert_empty();

    // A document with no pages.
    let (engine, state) = document_engine(PdfiumScript {
        page_count: 0,
        ..PdfiumScript::default()
    });
    assert!(
        engine
            .open(
                ordinary_pdf().into_boxed_slice(),
                &OpenOptions::new(Limits::default(), stopped())
            )
            .is_err()
    );
    state.assert_empty();

    // A page count over the limit -- the document existed and must not escape.
    let (engine, state) = document_engine(PdfiumScript {
        page_count: 50,
        ..PdfiumScript::default()
    });
    let limits = Limits::with(|l| l.max_pages = 2);
    assert!(matches!(
        engine.open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(limits, stopped())
        ),
        Err(Error::LimitExceeded {
            limit: "max_pages",
            ..
        })
    ));
    state.assert_empty();
}

/// The password copy in the engine heap is outside Rust's allocator, so `Zeroizing` cannot
/// reach it. It has to be wiped explicitly, and it has to be wiped *before* the function
/// returns rather than at some later cleanup.
#[test]
fn the_password_copy_in_the_engine_heap_is_wiped_and_freed() {
    let (engine, state) = document_engine(PdfiumScript::default());
    let password = Password::new(b"correct horse battery staple");
    let mut options = OpenOptions::new(Limits::default(), stopped());
    options.password = Some(&password);

    let doc = engine
        .open(ordinary_pdf().into_boxed_slice(), &options)
        .expect("should open");
    drop(doc);

    let wipes: Vec<_> = state
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::WipeAndFree(ptr, len) => Some((ptr, len)),
            _ => None,
        })
        .collect();
    assert_eq!(
        wipes.len(),
        1,
        "the password copy must be wiped exactly once"
    );
    // The wiped length covers the password and its NUL terminator.
    assert_eq!(
        usize::try_from(wipes[0].1).unwrap(),
        b"correct horse battery staple".len() + 1
    );
    // And the bytes really were zero when the buffer was released -- "the wipe was called"
    // would be the weaker claim.
    let wiped = state.last_wiped_contents().expect("a wipe was recorded");
    assert!(
        wiped.iter().all(|b| *b == 0),
        "the password was still readable in the engine heap when it was freed"
    );
    state.assert_empty();
}

/// The user's DOCUMENT is wiped out of the engine heap when it is released, not merely freed.
///
/// # Why this matters more than the password case it was modelled on
///
/// One worker serves many documents in a session — the host keeps it across operations and
/// only recycles it when a heap crosses `max_memory_bytes`. A plain `_free` hands the buffer
/// back to the module's free list **with the user's file still in it**, where it stays until
/// something else happens to allocate over it. So the previous document was sitting in the
/// engine heap while the next one was being processed.
///
/// Passwords were wiped here from the start, on both engines, for exactly that reason. The
/// document is the larger object and had the weaker treatment; that asymmetry is what this
/// test closes.
///
/// It asserts the BYTES, not the call. "`close_document` was invoked" would pass against a
/// bridge that ignored the length.
#[test]
fn the_document_is_wiped_out_of_the_engine_heap_when_it_is_released() {
    let (engine, state) = document_engine(PdfiumScript::default());
    let bytes = ordinary_pdf();
    let length = bytes.len();

    let doc = engine
        .open(
            bytes.into_boxed_slice(),
            &OpenOptions::new(Limits::default(), stopped()),
        )
        .expect("an ordinary document should open");
    drop(doc);

    let closes: Vec<u32> = state
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::CloseDocument(_, _, len) => Some(len),
            _ => None,
        })
        .collect();
    assert_eq!(closes.len(), 1, "the document is released exactly once");
    assert_eq!(
        usize::try_from(closes[0]).unwrap(),
        length,
        "the wipe must cover the whole input, not a truncated length"
    );

    let wiped = state.last_wiped_contents().expect("a wipe was recorded");
    assert!(
        wiped.iter().all(|b| *b == 0),
        "the user's document was still readable in the engine heap when it was released"
    );
    state.assert_empty();
}

/// The same, on the path where no document was ever attached.
///
/// A file PDFium refuses still reached the engine heap — it was copied in before the load was
/// attempted. That is the failure path, and a wipe that covered only the success path would
/// leave every rejected document behind.
#[test]
fn a_document_that_fails_to_load_is_wiped_too() {
    let (engine, state) = document_engine(PdfiumScript {
        load_succeeds: false,
        load_error_code: 3,
        ..PdfiumScript::default()
    });
    let bytes = ordinary_pdf();
    let length = bytes.len();

    let refused = engine.open(
        bytes.into_boxed_slice(),
        &OpenOptions::new(Limits::default(), stopped()),
    );
    assert!(refused.is_err(), "the script makes the load fail");

    let abandons: Vec<u32> = state
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::AbandonInput(_, len) => Some(len),
            _ => None,
        })
        .collect();
    assert_eq!(abandons.len(), 1, "the input is abandoned exactly once");
    assert_eq!(usize::try_from(abandons[0]).unwrap(), length);

    let wiped = state.last_wiped_contents().expect("a wipe was recorded");
    assert!(
        wiped.iter().all(|b| *b == 0),
        "a REFUSED document was left readable in the engine heap"
    );
    state.assert_empty();
}

/// The wipe happens before the load returns, not at the end of the open.
#[test]
fn the_password_is_wiped_immediately_after_the_load_not_at_the_end() {
    let (engine, state) = document_engine(PdfiumScript::default());
    let password = Password::new(b"hunter2");
    let mut options = OpenOptions::new(Limits::default(), stopped());
    options.password = Some(&password);
    let doc = engine
        .open(ordinary_pdf().into_boxed_slice(), &options)
        .expect("should open");

    let calls = state.calls();
    let load = calls
        .iter()
        .position(|c| matches!(c, Call::Load))
        .expect("loaded");
    let wipe = calls
        .iter()
        .position(|c| matches!(c, Call::WipeAndFree(..)))
        .expect("wiped");
    let page_count = calls
        .iter()
        .position(|c| matches!(c, Call::PageCount))
        .expect("counted");
    assert!(load < wipe, "the wipe must follow the load");
    assert!(
        wipe < page_count,
        "the password must be gone before any further engine work"
    );
    drop(doc);
}

/// A password with an interior NUL is refused before anything reaches the engine heap.
#[test]
fn a_password_with_a_nul_is_refused_before_the_engine_is_touched() {
    let (engine, state) = document_engine(PdfiumScript::default());
    let password = Password::new(b"before\0after");
    let mut options = OpenOptions::new(Limits::default(), stopped());
    options.password = Some(&password);

    let error = engine
        .open(ordinary_pdf().into_boxed_slice(), &options)
        .expect_err("a NUL should be refused");
    assert!(matches!(error, Error::InvalidArgument(_)), "{error:?}");
    assert!(
        state.calls().is_empty(),
        "nothing should have crossed the bridge: {:?}",
        state.calls()
    );
    let rendered = error.to_string();
    for leaked in ["before", "after"] {
        assert!(
            !rendered.contains(leaked),
            "{leaked:?} leaked into {rendered:?}"
        );
    }
}

/// The limits are enforced in the same order as the native path, and the early ones fire
/// before the engine is touched at all.
#[test]
fn the_input_size_limit_fires_before_anything_crosses_the_bridge() {
    let (engine, state) = document_engine(PdfiumScript::default());
    let bytes = ordinary_pdf();
    let requested = u64::try_from(bytes.len()).unwrap();
    let limits = Limits::with(|l| l.max_input_bytes = 16);

    match engine.open(
        bytes.into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded {
            limit,
            stage,
            requested: r,
            allowed,
        }) => {
            assert_eq!(limit, "max_input_bytes");
            // The SAME stage the native path reports for the same file. ROADMAP item 12's
            // harness compares this across the two implementations; asserting it here is
            // what stops the two drifting before the harness ever sees them.
            assert_eq!(stage, Stage::InputSize);
            assert_eq!(r, requested);
            assert_eq!(allowed, 16);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
    assert!(
        state.calls().is_empty(),
        "the engine should never have been called"
    );
}

/// The declared-size bomb is refused by the pre-scan, on the web as on native, and before
/// the bytes are copied into an engine heap that has a hard 2 GiB ceiling.
#[test]
fn the_declared_size_bomb_is_refused_before_the_bridge() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance/fixtures/xref-bomb.pdf");
    // `expect`, not a silent `return`. The fixture is committed (`tests/conformance/`), so a
    // missing one is a broken checkout, and a test that quietly becomes a no-op is exactly
    // the kind that stops protecting anything without anyone noticing.
    let bytes =
        std::fs::read(&path).expect("tests/conformance/fixtures/xref-bomb.pdf is committed");
    let (engine, state) = document_engine(PdfiumScript::default());
    match engine.open(
        bytes.into_boxed_slice(),
        &OpenOptions::new(Limits::default(), stopped()),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_memory_bytes"),
        other => panic!("the pre-scan should have refused the bomb, got {other:?}"),
    }
    assert!(
        state.calls().is_empty(),
        "the bomb must never reach the engine heap"
    );
}

/// The measured check catches growth the size estimate could not have predicted — the
/// after-the-fact half, reading the engine module's heap rather than a process RSS.
#[test]
fn growth_the_estimate_could_not_predict_is_caught_after_the_load() {
    let limits = Limits::with(|l| l.max_memory_bytes = 8 * 1024 * 1024);
    let (engine, state) = document_engine(PdfiumScript {
        // Far past the limit plus the noise margin, so this is unambiguous.
        load_grows_heap_by: 4 * 1024 * 1024 * 1024,
        ..PdfiumScript::default()
    });
    match engine.open(
        ordinary_pdf().into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_memory_bytes"),
        other => panic!("expected the measured check to fire, got {other:?}"),
    }
    state.assert_empty();
}

/// An allocation failure in the engine heap is `Io`, not a silent success and not a panic.
#[test]
fn an_engine_heap_allocation_failure_is_reported_not_ignored() {
    let (engine, state) = document_engine(PdfiumScript::default());
    state.fail_next_alloc();
    let error = engine
        .open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(Limits::default(), stopped()),
        )
        .expect_err("a failed allocation must not read as success");
    assert!(matches!(error, Error::Io(_)), "{error:?}");
    // On the web this is not a recoverable condition: wasm memory never shrinks, so a
    // module that could not allocate has reached its ceiling for the worker's life. The
    // binding classifies it fatal for that reason; see `burrow_wasm::is_fatal`.
    state.assert_empty();
}

/// ADR 0006 requirement 6, on the web path: a zero error code alongside a failed load must
/// never read as success. This is the `-0 === 0` bug the spike found, in its natural home.
#[test]
fn a_failed_load_reporting_no_error_code_is_never_success() {
    let (engine, state) = document_engine(PdfiumScript {
        load_succeeds: false,
        load_error_code: 0,
        ..PdfiumScript::default()
    });
    let error = engine
        .open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(Limits::default(), stopped()),
        )
        .expect_err("a null handle is a failure whatever the code says");
    assert!(matches!(error, Error::Internal(_)), "{error:?}");
    state.assert_empty();
}

/// The password code maps to `PasswordRequired` through the *shared* table, so the web and
/// native paths cannot disagree about it.
#[test]
fn the_password_code_maps_the_same_way_it_does_natively() {
    let (engine, _state) = document_engine(PdfiumScript {
        load_succeeds: false,
        load_error_code: 4, // FPDF_ERR_PASSWORD
        ..PdfiumScript::default()
    });
    assert!(matches!(
        engine.open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(Limits::default(), stopped())
        ),
        Err(Error::PasswordRequired)
    ));
}

#[test]
fn a_web_document_is_send_and_sync() {
    // The trait requires it, and a handle that is a number rather than a pointer is what
    // makes it true without any `unsafe impl`.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<super::WebDocument>();
}

// ---------------------------------------------------------------------------------
// qpdf
// ---------------------------------------------------------------------------------

#[test]
fn a_structure_check_reports_the_page_count() {
    let (engine, state) = structure_engine(QpdfScript {
        page_count: 12,
        ..QpdfScript::default()
    });
    let report = engine
        .check(
            ordinary_pdf().into_boxed_slice(),
            &CheckOptions::new(Limits::default(), stopped()),
        )
        .expect("should check out");
    assert_eq!(report.pages, 12);
    state.assert_empty();
}

/// qpdf is silenced **before** it is handed anything to complain about.
///
/// The order is the whole point: `qpdf_read_memory` is one of the calls that warns, and a
/// warning that reaches a browser console carries object numbers and byte offsets.
#[test]
fn qpdf_is_silenced_before_it_reads_anything() {
    let (engine, state) = structure_engine(QpdfScript::default());
    let _ = engine.check(
        ordinary_pdf().into_boxed_slice(),
        &CheckOptions::new(Limits::default(), stopped()),
    );
    let calls = state.calls();

    let silence = calls
        .iter()
        .position(|c| matches!(c, Call::SilenceErrors))
        .expect("silenced");
    let suppress = calls
        .iter()
        .position(|c| matches!(c, Call::SuppressWarnings(true)))
        .expect("warnings suppressed");
    let logger = calls
        .iter()
        .position(|c| matches!(c, Call::SetLogger))
        .expect("logger set");
    let read = calls
        .iter()
        .position(|c| matches!(c, Call::ReadMemory))
        .expect("read");

    assert!(silence < read, "qpdf_silence_errors must precede the read");
    assert!(suppress < read, "warning suppression must precede the read");
    assert!(logger < read, "the discarding logger must precede the read");
}

/// `QPDF_ERROR_CODE` is a bitmask. A warnings-only status must not read as a failure —
/// the same trap the native path has a named test for, now covered on both paths by one
/// shared helper.
#[test]
fn a_warnings_only_status_is_not_a_failure_on_the_web_path_either() {
    let (engine, state) = structure_engine(QpdfScript {
        read_status: 1 << 0, // QPDF_WARNINGS, with no QPDF_ERRORS bit
        page_count: 3,
        ..QpdfScript::default()
    });
    let report = engine
        .check(
            ordinary_pdf().into_boxed_slice(),
            &CheckOptions::new(Limits::default(), stopped()),
        )
        .expect("warnings alone must not fail the read");
    assert_eq!(report.pages, 3);
    state.assert_empty();
}

#[test]
fn an_error_bit_in_the_read_status_is_a_failure() {
    let (engine, state) = structure_engine(QpdfScript {
        read_status: 1 << 1,    // QPDF_ERRORS
        pending_error: Some(4), // qpdf_e_password
        ..QpdfScript::default()
    });
    assert!(matches!(
        engine.check(
            ordinary_pdf().into_boxed_slice(),
            &CheckOptions::new(Limits::default(), stopped())
        ),
        Err(Error::PasswordRequired)
    ));
    state.assert_empty();
}

/// The error slot is drained before `qpdf_cleanup`, so qpdf never writes
/// `"application did not handle error: <text>"` — byte offsets and all — to the default
/// logger, which the per-document discarding logger does not cover.
///
/// # Why this test needs an error that *reappears*
///
/// A first version of this test set a pending error and asserted the check failed. It
/// passed with the drain loop deleted, because every path through `check` already calls
/// `take_error` before returning — which is exactly what the native code's comment says:
/// the drain in `Drop` is "unreachable today", a property of the control flow rather than
/// of the type. A test that only exercises reachable paths cannot see it.
///
/// So the fake re-arms the slot once, after the first drain, modelling qpdf recording a
/// further problem later. That is the only shape in which the slot is still occupied when
/// `Drop` runs. The re-arm is one-shot, so the test also pins that the loop terminates.
///
/// The assertion itself lives in the fake's `cleanup`, which panics rather than silently
/// emitting the warning — so this invariant is enforced by *every* qpdf test here, not
/// just by this one remembering to check.
#[test]
fn an_error_that_appears_after_the_last_check_is_still_drained_before_cleanup() {
    let (engine, state) = structure_engine(QpdfScript {
        read_status: 0,
        pending_error: Some(5),
        error_reappears_once: Some(5),
        ..QpdfScript::default()
    });
    let result = engine.check(
        ordinary_pdf().into_boxed_slice(),
        &CheckOptions::new(Limits::default(), stopped()),
    );
    assert!(
        result.is_err(),
        "a pending error must not be reported as success"
    );

    // Reaching here at all means `cleanup` saw an empty slot: the fake panics otherwise.
    assert!(
        state.calls().iter().any(|c| matches!(c, Call::Cleanup)),
        "cleanup should have run"
    );
    state.assert_empty();
}

#[test]
fn the_qpdf_buffer_is_freed_on_every_failure_path() {
    for script in [
        QpdfScript {
            read_status: 1 << 1,
            pending_error: Some(5),
            ..QpdfScript::default()
        },
        QpdfScript {
            page_count: -1,
            pending_error: Some(6),
            ..QpdfScript::default()
        },
    ] {
        let (engine, state) = structure_engine(script);
        let _ = engine.check(
            ordinary_pdf().into_boxed_slice(),
            &CheckOptions::new(Limits::default(), stopped()),
        );
        state.assert_empty();
    }
}

#[test]
fn a_failed_qpdf_init_is_internal_rather_than_a_panic() {
    let (engine, state) = structure_engine(QpdfScript {
        init_succeeds: false,
        ..QpdfScript::default()
    });
    let error = engine
        .check(
            ordinary_pdf().into_boxed_slice(),
            &CheckOptions::new(Limits::default(), stopped()),
        )
        .expect_err("a null handle must be reported");
    assert!(matches!(error, Error::Internal(_)), "{error:?}");
    state.assert_empty();
}

#[test]
fn too_many_pages_is_a_limit_error_on_the_web_path_too() {
    let (engine, state) = structure_engine(QpdfScript {
        page_count: 5,
        ..QpdfScript::default()
    });
    let limits = Limits::with(|l| l.max_pages = 2);
    match engine.check(
        ordinary_pdf().into_boxed_slice(),
        &CheckOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded {
            limit, requested, ..
        }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(requested, 5);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
    state.assert_empty();
}

/// qpdf's process-global resource limits are applied on the web path, from the same table
/// the native path uses.
///
/// They were not applied at all before, and that mattered more here than on native: every
/// decompression memory limit qpdf offers defaults to **unlimited**, an allocator giving up
/// inside C++ is an `abort()`, and an Emscripten `abort()` is quiet — it leaves the worker
/// alive with its init flags set, so one crafted file bricks the engine for the session.
#[test]
fn the_global_resource_limits_are_applied_on_the_web_path_too() {
    let (engine, state) = structure_engine(QpdfScript::default());
    let _ = engine.check(
        ordinary_pdf().into_boxed_slice(),
        &CheckOptions::new(Limits::default(), stopped()),
    );

    let applied: Vec<_> = state
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::GlobalSet(param, value) => Some((param, value)),
            _ => None,
        })
        .collect();

    // The same list, not a copy of it: if the policy gains a parameter, this follows.
    let expected = crate::codes::qpdf::policy::settings();
    assert_eq!(
        applied,
        expected.to_vec(),
        "the web path must apply exactly the shared policy"
    );
    assert!(
        !applied.is_empty(),
        "an empty policy would make this test vacuous"
    );
}

/// The globals and the logger are installed once per engine, not once per operation.
///
/// The logger is the reason this matters. `qpdflogger_cleanup` is declared on neither path,
/// so a logger created per `check()` is a permanent allocation in a heap that never shrinks
/// — unbounded growth that no `Limits` covers. The native path avoids it with a
/// process-wide `OnceLock`; this is the web equivalent.
#[test]
fn the_logger_and_the_globals_are_installed_once_not_once_per_operation() {
    let (engine, state) = structure_engine(QpdfScript::default());
    for _ in 0..5 {
        let _ = engine.check(
            ordinary_pdf().into_boxed_slice(),
            &CheckOptions::new(Limits::default(), stopped()),
        );
    }

    let loggers = state
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::LoggerCreate))
        .count();
    assert_eq!(loggers, 1, "five operations created {loggers} loggers");

    let installs = state
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::GlobalSet(..)))
        .count();
    assert_eq!(
        installs,
        crate::codes::qpdf::policy::settings().len(),
        "the global limits were applied more than once"
    );

    // The fake allocates the logger through its tracked heap, as a *retained* allocation:
    // it is meant to outlive an operation, so the property is "exactly one", not "freed".
    // Five operations creating five loggers is the unbounded growth this guards against.
    assert_eq!(
        state.retained(),
        1,
        "five operations left {} long-lived allocations",
        state.retained()
    );
    state.assert_empty();
}

/// The web qpdf path applies the size estimate, since #26 --- and this test asserted the
/// opposite until it was measured.
///
/// It was called `a_file_under_the_memory_ceiling_is_checked_rather_than_estimated_away`,
/// and it pinned a decision taken on a reasonable but unverified premise: that
/// `crate::estimate`'s PDFium-derived constants "would reject files qpdf handles
/// comfortably". `examples/measure-open-cost.rs` says otherwise --- qpdf is the hungrier
/// engine, and on a 1 MB document of 9,000 pages it peaks at 19.30 MB, EXCEEDING the
/// 18.15 MB estimate, while PDFium uses 3.39 MB. The estimate is not too strict for qpdf.
///
/// The old test's other half is preserved and is why the fix was to ADD rather than to
/// remove: web and native must not disagree. `tests/limits.rs` asserts they refuse at the
/// same input length, with the same numbers.
#[test]
fn the_size_estimate_runs_on_the_web_qpdf_path_too() {
    let bytes = ordinary_pdf();
    // A ceiling above the input and below the estimate, which is the window the old
    // behaviour let through.
    let limits = Limits::with(|l| l.max_memory_bytes = u64::try_from(bytes.len()).unwrap() + 16);

    let (engine, state) = structure_engine(QpdfScript {
        page_count: 2,
        ..QpdfScript::default()
    });
    let refused = engine.check(
        bytes.into_boxed_slice(),
        &CheckOptions::new(limits, stopped()),
    );
    assert!(
        matches!(
            &refused,
            Err(Error::LimitExceeded { limit, stage, .. })
                if *limit == "max_memory_bytes" && stage.as_str() == "size_estimate"
        ),
        "got {refused:?}"
    );
    // AND IT REFUSED BEFORE THE ENGINE WAS TOUCHED. That is the whole value of a
    // length-based pre-check: it costs nothing and nothing has been allocated yet.
    assert!(
        state.calls().is_empty(),
        "the estimate refused only after crossing the bridge: {:?}",
        state.calls()
    );
    state.assert_empty();
}

/// The measured memory check runs on the qpdf path too.
///
/// `QpdfBridge::heap_bytes` was declared and called from nowhere — a method that existed to
/// make the two bridges look symmetric, in a trait whose own docs describe the method list
/// as the audit surface. Where the size estimate predicts from the input's length, this one
/// reads what the engine actually did --- and since #26 both run on this path.
#[test]
fn a_read_that_costs_more_than_allowed_is_caught_on_the_qpdf_path() {
    let limits = Limits::with(|l| l.max_memory_bytes = 8 * 1024 * 1024);
    let (engine, state) = structure_engine(QpdfScript {
        // Far past the ceiling plus the noise margin, so this is unambiguous.
        read_grows_heap_by: 4 * 1024 * 1024 * 1024,
        ..QpdfScript::default()
    });
    match engine.check(
        ordinary_pdf().into_boxed_slice(),
        &CheckOptions::new(limits, stopped()),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_memory_bytes"),
        other => panic!("expected the measured check to fire, got {other:?}"),
    }
    state.assert_empty();
}

/// `max_duration_ms` is enforced on the web path, at the checkpoints the native path uses.
///
/// The one `Limits` field the fake-bridge suite did not exercise: every other test here runs
/// on a stopped `ManualClock`, so the deadline could have been removed entirely without any
/// of them noticing. `page_count` deliberately takes no clock — PR 2 found that a second
/// clock silently disabled the limit — so this advances the one the document was opened
/// with.
#[test]
fn a_deadline_that_passes_between_calls_is_caught_on_the_web_path() {
    let clock = Arc::new(ManualClock::new(0));
    let limits = Limits::with(|l| l.max_duration_ms = 1_000);
    let (engine, state) = document_engine(PdfiumScript::default());

    let doc = engine
        .open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>),
        )
        .expect("the budget has not been spent yet");

    // Still inside the budget.
    assert!(engine.page_count(&doc).is_ok());

    clock.advance(1_001);
    match engine.page_count(&doc) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_duration_ms"),
        other => panic!("expected the deadline to fire, got {other:?}"),
    }
    drop(doc);
    state.assert_empty();
}

/// A clock that advances every time it is read.
///
/// `ManualClock` is the right tool when a test can advance time between calls, as the PDFium
/// test above does. `check()` is a single call, so its two checkpoints bracket work that a
/// stopped clock makes instantaneous — this makes time pass *inside* the operation, which is
/// what a slow engine actually looks like.
struct TickingClock {
    now: std::sync::atomic::AtomicU64,
    step: u64,
}

impl Clock for TickingClock {
    fn now_ms(&self) -> u64 {
        self.now
            .fetch_add(self.step, std::sync::atomic::Ordering::Relaxed)
    }
}

/// The structure engine's deadline fires when the work outlasts the budget.
///
/// A first version set `max_duration_ms = 0` against a stopped clock and expected a failure.
/// It did not fail, and the code was right: zero elapsed does not *exceed* a zero budget.
/// The test premise was wrong, not the deadline.
#[test]
fn a_check_that_outlasts_its_budget_is_stopped_on_the_web_path() {
    let clock: Arc<dyn Clock> = Arc::new(TickingClock {
        now: std::sync::atomic::AtomicU64::new(0),
        step: 1_000,
    });
    let limits = Limits::with(|l| l.max_duration_ms = 500);
    let (engine, state) = structure_engine(QpdfScript::default());

    match engine.check(
        ordinary_pdf().into_boxed_slice(),
        &CheckOptions::new(limits, clock),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_duration_ms"),
        other => panic!("expected the deadline to fire, got {other:?}"),
    }
    state.assert_empty();
}

/// And a generous budget is not tripped by the same clock, so the test above is measuring
/// the budget rather than the clock.
#[test]
fn a_check_inside_its_budget_is_not_stopped() {
    let clock: Arc<dyn Clock> = Arc::new(TickingClock {
        now: std::sync::atomic::AtomicU64::new(0),
        step: 1_000,
    });
    let limits = Limits::with(|l| l.max_duration_ms = 1_000_000);
    let (engine, state) = structure_engine(QpdfScript {
        page_count: 4,
        ..QpdfScript::default()
    });

    let report = engine
        .check(
            ordinary_pdf().into_boxed_slice(),
            &CheckOptions::new(limits, clock),
        )
        .expect("a generous budget must not fire");
    assert_eq!(report.pages, 4);
    state.assert_empty();
}

// ---------------------------------------------------------------- the assembler
//
// M1 PR B2. These cover what Playwright cannot see from outside a worker: the ORDER of the
// bridge calls, when each engine-heap buffer is released, and which of qpdf's two ways of
// reporting "no output" the code actually checks.

fn merge_options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, stopped())
}

#[test]
fn the_assembler_writes_after_it_prepares_and_copies_out_once() {
    // THE ORDERING THAT COST A CORE DUMP ON THE NATIVE PATH, asserted here rather than
    // described: `set_deterministic_id` must come AFTER `init_write_memory`, because the
    // writer does not exist until then.
    let (engine, state) = structure_engine(QpdfScript::default());
    let mut assembly = engine
        .begin(
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("begin");
    engine
        .append(
            &mut assembly,
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("append");
    let out = engine.finish(assembly).expect("finish");

    assert_eq!(
        out, b"%PDF-1.7\nmerged\n",
        "the bytes qpdf produced did not reach Rust"
    );

    let calls = state.calls();
    let write_path: Vec<&Call> = calls
        .iter()
        .filter(|c| {
            matches!(
                c,
                Call::InitWriteMemory
                    | Call::SetDeterministicId(_)
                    | Call::Write
                    | Call::GetBufferLength
                    | Call::GetBuffer
                    | Call::CopyOut(_)
            )
        })
        .collect();
    assert_eq!(
        write_path,
        vec![
            &Call::InitWriteMemory,
            &Call::SetDeterministicId(true),
            &Call::Write,
            &Call::GetBufferLength,
            &Call::GetBuffer,
            &Call::CopyOut(16),
        ],
        "the write path ran out of order: {calls:?}"
    );
}

#[test]
fn every_page_of_every_source_is_appended_in_order() {
    let (engine, state) = structure_engine(QpdfScript {
        page_count: 3,
        ..QpdfScript::default()
    });
    let mut assembly = engine
        .begin(
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("begin");
    let appended = engine
        .append(
            &mut assembly,
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("append");
    assert_eq!(appended, 3);

    let calls = state.calls();
    let pages: Vec<&Call> = calls
        .iter()
        .filter(|c| matches!(c, Call::GetPageN(_) | Call::AddPage { .. }))
        .collect();
    assert_eq!(
        pages,
        vec![
            &Call::GetPageN(0),
            &Call::AddPage {
                page: 1,
                first: false
            },
            &Call::GetPageN(1),
            &Call::AddPage {
                page: 2,
                first: false
            },
            &Call::GetPageN(2),
            &Call::AddPage {
                page: 3,
                first: false
            },
        ],
        "pages were appended out of order, or a handle was not the one just fetched"
    );
    let _ = engine.finish(assembly);
}

#[test]
fn nothing_is_prepended() {
    // `first: true` would silently reverse the document. The fake records the flag so this
    // is assertable rather than a matter of reading the call site.
    let (engine, state) = structure_engine(QpdfScript::default());
    let mut assembly = engine
        .begin(
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("begin");
    engine
        .append(
            &mut assembly,
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("append");
    assert!(
        !state
            .calls()
            .iter()
            .any(|c| matches!(c, Call::AddPage { first: true, .. })),
        "a page was prepended, which reverses the merge"
    );
    let _ = engine.finish(assembly);
}

#[test]
fn a_warning_from_add_page_is_not_a_failure() {
    // QPDF_WARNINGS is bit 0. `!= 0` here would reject every damaged-but-readable input.
    let (engine, _state) = structure_engine(QpdfScript {
        add_page_status: 1,
        ..QpdfScript::default()
    });
    let mut assembly = engine
        .begin(
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("begin");
    engine
        .append(
            &mut assembly,
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("a warnings-only add_page must not fail");
    let _ = engine.finish(assembly);
}

#[test]
fn an_error_from_add_page_is_a_failure() {
    // The control for the test above: bit 1 is QPDF_ERRORS and must fail.
    let (engine, _state) = structure_engine(QpdfScript {
        add_page_status: 2,
        ..QpdfScript::default()
    });
    let mut assembly = engine
        .begin(
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("begin");
    let err = engine
        .append(
            &mut assembly,
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect_err("an error bit must fail");
    assert!(matches!(err, Error::Malformed(_)), "got {err:?}");
    let _ = engine.finish(assembly);
}

#[test]
fn a_null_buffer_is_refused_even_when_a_length_is_reported() {
    // qpdf reports the buffer and its length through SEPARATE accessors, so they can
    // disagree. A caller that trusted only the length would read from null.
    let (engine, _state) = structure_engine(QpdfScript {
        buffer_is_null: true,
        ..QpdfScript::default()
    });
    let assembly = engine
        .begin(
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("begin");
    let err = engine
        .finish(assembly)
        .expect_err("a null buffer must fail");
    assert!(matches!(err, Error::Io(_)), "got {err:?}");
}

#[test]
fn an_empty_output_is_refused() {
    let (engine, _state) = structure_engine(QpdfScript {
        output: Vec::new(),
        ..QpdfScript::default()
    });
    let assembly = engine
        .begin(
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        )
        .expect("begin");
    let err = engine.finish(assembly).expect_err("zero bytes must fail");
    assert!(matches!(err, Error::Io(_)), "got {err:?}");
}

#[test]
fn the_page_ceiling_is_on_the_output_total() {
    // Two sources of three pages each against a five-page ceiling. Per-input checking would
    // pass both.
    let (engine, state) = structure_engine(QpdfScript {
        page_count: 3,
        ..QpdfScript::default()
    });
    let limits = Limits::with(|l| l.max_pages = 5);
    let mut assembly = engine
        .begin(ordinary_pdf().into_boxed_slice(), &merge_options(limits))
        .expect("begin");
    let err = engine
        .append(
            &mut assembly,
            ordinary_pdf().into_boxed_slice(),
            &merge_options(limits),
        )
        .expect_err("6 pages against a 5-page ceiling must fail");

    match err {
        Error::LimitExceeded {
            limit,
            stage,
            requested,
            allowed,
        } => {
            assert_eq!(limit, "max_pages");
            assert_eq!(stage, Stage::PageCount);
            assert_eq!(requested, 6, "the output total, not one input");
            assert_eq!(allowed, 5);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
    assert!(
        !state
            .calls()
            .iter()
            .any(|c| matches!(c, Call::AddPage { .. })),
        "pages were copied before the ceiling was checked"
    );
    let _ = engine.finish(assembly);
}

#[test]
fn every_engine_heap_allocation_is_released_when_the_merge_finishes() {
    // THE PROPERTY PLAYWRIGHT CANNOT SEE. Each source holds its input buffer in the engine
    // heap until `finish`, which is required for correctness -- qpdf reads them during the
    // write -- so the thing to assert is that they are all gone AFTERWARDS.
    let state = {
        let (engine, state) = structure_engine(QpdfScript::default());
        let mut assembly = engine
            .begin(
                ordinary_pdf().into_boxed_slice(),
                &merge_options(Limits::default()),
            )
            .expect("begin");
        for _ in 0..3 {
            engine
                .append(
                    &mut assembly,
                    ordinary_pdf().into_boxed_slice(),
                    &merge_options(Limits::default()),
                )
                .expect("append");
        }
        engine.finish(assembly).expect("finish");
        state
    };
    state.assert_empty();
    assert_eq!(
        state.retained(),
        1,
        "there must be exactly one retained allocation -- the shared discarding logger -- \
         however many documents the merge opened"
    );
}

#[test]
fn a_failed_append_still_releases_what_it_allocated() {
    let state = {
        let (engine, state) = structure_engine(QpdfScript {
            add_page_status: 2,
            ..QpdfScript::default()
        });
        let mut assembly = engine
            .begin(
                ordinary_pdf().into_boxed_slice(),
                &merge_options(Limits::default()),
            )
            .expect("begin");
        let _ = engine.append(
            &mut assembly,
            ordinary_pdf().into_boxed_slice(),
            &merge_options(Limits::default()),
        );
        drop(assembly);
        state
    };
    state.assert_empty();
}

// ---------------------------------------------------------------------------------------
// rotate, over the fake bridge
//
// THESE ARE THE TESTS `web/rotate.rs`'s HEADER ALREADY CLAIMED EXISTED. It said "`web/tests.rs`
// counts" while nothing counted, and `QpdfScript::live_handles` was written by three call
// sites and read by none -- a counter that reports "no leak" for every input. Both reviewers
// found it; the security reviewer also found that the fake could not have run these at all,
// because `read_c_string` walked the heap byte by byte and `Heap::read` panics on anything but
// an exact base address.
//
// What they catch that the conformance corpus cannot: the corpus fixes rotate at "every page,
// by 90", so writing `/Rotate` to the shared `/Pages` ancestor instead of to each page produces
// exactly the expected vector. That is hazard #2 in both module headers -- the one that rotates
// a whole document while reporting success on one page -- and only an assertion about WHICH
// handle was written can see it.

/// A script whose page tree inherits: no `/Rotate` on the page, a dictionary `/Parent` above.
fn inheriting_script() -> QpdfScript {
    QpdfScript {
        page_count: 3,
        oh_type_codes: [
            ("/Rotate".to_owned(), 2), // ot_null -- absent on the page
            ("/Parent".to_owned(), 9), // ot_dictionary
            ("<page 0>".to_owned(), 9),
            ("<page 1>".to_owned(), 9),
            ("<page 2>".to_owned(), 9),
        ]
        .into_iter()
        .collect(),
        ..QpdfScript::default()
    }
}

fn rotate_options() -> OpenOptions<'static> {
    OpenOptions::new(Limits::DEFAULT, stopped())
}

#[test]
fn the_rotation_is_written_to_the_page_and_never_to_an_ancestor() {
    // THE ASSERTION THE CORPUS STRUCTURALLY CANNOT MAKE. Every page is selected there, so an
    // ancestor write is observationally identical to a per-page write; here the recorded call
    // names the handle, and the page's handle is not the one the `/Parent` walk produced.
    let (engine, state) = structure_engine(QpdfScript {
        // A `/Rotate` on the ancestor, absent on the page -- so the walk climbs, finds 90, and
        // the write has an ancestor handle sitting right there to be aimed at by mistake.
        oh_int_value: 90,
        oh_type_codes: [
            ("/Rotate".to_owned(), 4), // ot_integer, found on whichever node is asked
            ("/Parent".to_owned(), 9),
        ]
        .into_iter()
        .collect(),
        page_count: 3,
        ..QpdfScript::default()
    });

    let source = PageRotator::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");
    let _ = engine.rotate(&source, &[0], Rotation::Clockwise90, &rotate_options());

    let calls = state.calls();
    let page_handle = calls.iter().find_map(|c| match c {
        Call::GetPageN(0) => Some(()),
        _ => None,
    });
    assert!(page_handle.is_some(), "the page was never fetched");

    // The handle `get_page_n` issued is the first one the fake hands out for this document.
    let replaced: Vec<u32> = calls
        .iter()
        .filter_map(|c| match c {
            Call::OhReplaceKey { oh, key, .. } if key == "/Rotate" => Some(*oh),
            _ => None,
        })
        .collect();
    assert_eq!(replaced.len(), 1, "exactly one /Rotate write, on one page");

    // The write went to the handle `get_page_n` produced, which is handle 1 -- the first the
    // fake issues. An ancestor write would name a later one.
    // MEASURED, not assumed: aiming the write at the handle the `/Parent` lookup produces
    // makes this fail with `left: 4, right: 1`. Without that check this assertion is a
    // constant compared against a constant.
    assert_eq!(
        replaced[0], 1,
        "/Rotate was written to an ancestor handle, not to the page: this rotates every page \
         under that node and reports success"
    );
}

#[test]
fn every_object_handle_the_web_rotation_takes_is_released() {
    // The count the module header promised. `live_handles` is decremented by `oh_release` and
    // incremented by every handle the fake issues, and `oh_release` now refuses a handle it
    // never issued -- so a double release cannot cancel out a leak and leave this at zero.
    let script = inheriting_script();
    let live = Arc::clone(&script.live_handles);
    let (engine, _state) = structure_engine(script);

    let source = PageRotator::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");

    let _ = engine.rotate(
        &source,
        &[0, 1, 2],
        Rotation::Clockwise90,
        &rotate_options(),
    );
    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "the rotation left object handles alive in qpdf's cache"
    );
}

#[test]
fn a_refused_rotation_releases_its_handles_too() {
    // THE ERROR PATHS, which is where a hand-written release is actually lost -- `?` returns
    // past it and nothing complains. The refusal here is a page past the end, raised after the
    // walk has already taken handles for the pages before it.
    let script = inheriting_script();
    let live = Arc::clone(&script.live_handles);
    let (engine, _state) = structure_engine(script);

    let source = PageRotator::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");

    let refused = engine.rotate(&source, &[0, 9], Rotation::Clockwise90, &rotate_options());
    assert!(matches!(refused, Err(Error::InvalidArgument(_))));
    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "a refused rotation left object handles alive"
    );
}

#[test]
fn a_parent_that_never_ends_is_refused_rather_than_walked_forever() {
    // The fake's `/Parent` is a dictionary for ever, which is a `/Parent` cycle by another
    // name: the walk can never reach the top. It must exhaust `MAX_PAGE_TREE_DEPTH` and refuse
    // rather than spin -- and release every handle it took on the way.
    let script = inheriting_script();
    let live = Arc::clone(&script.live_handles);
    let (engine, _state) = structure_engine(script);

    let source = PageRotator::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");

    assert!(matches!(
        engine.effective_rotation(&source, 0),
        Err(Error::Malformed(_))
    ));
    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "the depth-exhausted walk left handles alive"
    );
}

#[test]
fn a_rotate_that_is_not_an_integer_is_malformed_on_the_web_too() {
    // TRAPPING ANSWERS CRASHES, NOT WRONG ANSWERS -- the same rule as the native path, asserted
    // separately because the two implementations are separate on purpose. No corpus fixture has
    // a `/Rotate` of the wrong type.
    let (engine, _state) = structure_engine(QpdfScript {
        oh_type_codes: [
            ("/Rotate".to_owned(), 7), // ot_name
            ("/Parent".to_owned(), 9),
        ]
        .into_iter()
        .collect(),
        ..QpdfScript::default()
    });

    let source = PageRotator::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");
    assert!(matches!(
        engine.effective_rotation(&source, 0),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_rotation_is_relative_to_the_page_s_own_value_on_the_web_too() {
    // `/Rotate 90` on the page plus a 90 turn writes 180. The case where `oh_int_value` is
    // actually read, and where a version that ignored the current value would write 90.
    let (engine, state) = structure_engine(QpdfScript {
        oh_int_value: 90,
        oh_type_codes: [("/Rotate".to_owned(), 4), ("/Parent".to_owned(), 9)]
            .into_iter()
            .collect(),
        ..QpdfScript::default()
    });

    let source = PageRotator::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");
    let _ = engine.rotate(&source, &[0], Rotation::Clockwise90, &rotate_options());

    let written: Vec<i64> = state
        .calls()
        .iter()
        .filter_map(|c| match c {
            Call::OhNewInteger(v) => Some(*v),
            _ => None,
        })
        .collect();
    assert_eq!(written, vec![180], "90 inherited plus a 90 turn is 180");
}

// --- reorder, over the bridge ------------------------------------------------------------

/// The fake's engine, with `pages` pages and nothing unusual scripted.
fn reorder_engine(pages: i32) -> (WebQpdf, Arc<FakeQpdf>) {
    let state = FakeHeap::new();
    let bridge = Arc::new(FakeQpdf::new(
        state,
        QpdfScript {
            page_count: pages,
            ..QpdfScript::default()
        },
    ));
    (
        WebQpdf::new(Arc::clone(&bridge) as Arc<dyn QpdfBridge>),
        bridge,
    )
}

fn reordered(engine: &WebQpdf, pages: u64, order: &[u64]) -> Result<Vec<u8>> {
    let source = PageReorderer::open(engine, ordinary_pdf().into_boxed_slice(), &rotate_options())?;
    let permutation = Permutation::of(order.to_vec(), pages)?;
    engine.reorder(&source, &permutation, &rotate_options())
}

#[test]
fn the_web_reorder_puts_the_pages_in_the_order_it_was_given() {
    // THE ASSERTION THAT MATTERS, and it is on the resulting ORDER rather than on the calls.
    // A reorder that made every expected bridge call and inserted each page one position off
    // would pass a call-shape assertion and produce the wrong document.
    //
    // The fake models `/Kids` as a vector of object numbers: `get_page_n` hands out a fresh
    // handle mapped to a stable object, `remove_page` takes that object out, `add_page_at`
    // puts it back. So this reads the document's order, not a log.
    let (engine, bridge) = reorder_engine(5);
    reordered(&engine, 5, &[4, 0, 2, 3, 1]).expect("the fake reorders");
    assert_eq!(
        bridge.page_order(),
        vec![5, 1, 3, 4, 2],
        "the pages are not in the order the permutation named"
    );
}

#[test]
fn the_web_reorder_reverses_correctly() {
    let (engine, bridge) = reorder_engine(4);
    reordered(&engine, 4, &[3, 2, 1, 0]).expect("the fake reorders");
    assert_eq!(bridge.page_order(), vec![4, 3, 2, 1]);
}

#[test]
fn the_web_identity_permutation_moves_nothing() {
    // Not merely "the order is unchanged" -- that is true of a no-op and of a reorder that
    // moved every page and put it back. The CALLS are what separate them, so both are
    // asserted: nothing moved, and the page machinery was never entered.
    let (engine, bridge) = reorder_engine(4);
    reordered(&engine, 4, &[0, 1, 2, 3]).expect("the fake reorders");
    assert_eq!(bridge.page_order(), vec![1, 2, 3, 4]);
    assert!(
        !bridge
            .state
            .calls()
            .iter()
            .any(|c| matches!(c, Call::RemovePage(_) | Call::AddPageAt { .. })),
        "the identity permutation entered qpdf's page machinery, which is what flattens the \
         page tree (ADR 0021)"
    );
}

#[test]
fn a_page_already_in_place_is_not_moved() {
    // The partial reorder: only pages 1 and 2 swap, so pages 0 and 3 must be left alone. A
    // comparison that always said "different" -- which is what comparing HANDLES does -- would
    // move all four and still produce the right order, so this asserts the call count too.
    let (engine, bridge) = reorder_engine(4);
    reordered(&engine, 4, &[0, 2, 1, 3]).expect("the fake reorders");
    assert_eq!(bridge.page_order(), vec![1, 3, 2, 4]);
    let moves = bridge
        .state
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::RemovePage(_)))
        .count();
    assert_eq!(
        moves, 1,
        "only one page needed to move; moving more means the identity comparison is always \
         false, which is what comparing raw handles does"
    );
}

#[test]
fn the_web_reorder_compares_objects_and_not_handles() {
    // THE DEFECT, ASSERTED DIRECTLY. The fake issues a fresh handle from every `get_page_n`,
    // exactly as qpdf does, so the handles for "the page that belongs here" and "the page
    // that is here" are never equal even when they are the same page. If this module compared
    // handles, the identity permutation above would move all four pages -- and it does not.
    //
    // This test pins the mechanism rather than the outcome: `oh_object` must be asked, twice
    // per page, or the comparison cannot be about identity at all.
    //
    // NOT the identity permutation, which is what this test asked for first: it
    // short-circuits before `permute` and makes no comparisons at all, so the assertion read
    // `left: 0`. The test was wrong and the code was right -- and the short-circuit being
    // observable here is itself worth having, since `the_web_identity_permutation_moves_nothing`
    // is the test that pins it.
    let (engine, bridge) = reorder_engine(3);
    reordered(&engine, 3, &[1, 0, 2]).expect("the fake reorders");
    let asked = bridge
        .state
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::OhObject(_)))
        .count();
    assert_eq!(
        asked, 6,
        "object identity must be read for both sides of every comparison"
    );
}

#[test]
fn every_object_handle_the_web_reorder_takes_is_released() {
    // `reorder` holds EVERY page's handle for the whole permutation, which is the thing
    // `rotate` does not do -- so the guard that releases them is this module's own and needs
    // its own measurement.
    let state = FakeHeap::new();
    let script = QpdfScript {
        page_count: 5,
        ..QpdfScript::default()
    };
    let live = Arc::clone(&script.live_handles);
    let bridge = Arc::new(FakeQpdf::new(state, script));
    let engine = WebQpdf::new(bridge as Arc<dyn QpdfBridge>);

    reordered(&engine, 5, &[4, 3, 2, 1, 0]).expect("the fake reorders");
    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "the reorder left object handles alive in qpdf's cache"
    );
}

#[test]
fn a_refused_web_reorder_releases_its_handles_too() {
    // THE ERROR PATH, and the first version of this test did not reach it.
    //
    // It scripted `pending_error`, which `Session::open` drains before a single handle is
    // taken -- so `live_handles == 0` because none had ever been issued, and emptying
    // `Pages::drop` left the test green. Code review instrumented the fake's call log and
    // found it ended at `Cleanup` with no `GetPageN` at all.
    //
    // The failure now lands INSIDE the permutation: `get_page_n` starts failing after three
    // calls, so three handles are live when the error is raised and the guard is the only
    // thing that can release them.
    let state = FakeHeap::new();
    let script = QpdfScript {
        page_count: 5,
        get_page_n_fails_after: Some(3),
        ..QpdfScript::default()
    };
    let live = Arc::clone(&script.live_handles);
    let bridge = Arc::new(FakeQpdf::new(Arc::clone(&state), script));
    let engine = WebQpdf::new(Arc::clone(&bridge) as Arc<dyn QpdfBridge>);

    let refused = reordered(&engine, 5, &[4, 3, 2, 1, 0]);
    assert!(refused.is_err(), "the scripted failure must propagate");

    // AND IT FAILED WHERE THE TEST SAYS IT DID. Without this the test could silently regress
    // to the shape it had -- refusing at `open`, releasing nothing, and passing.
    let took = state
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::GetPageN(_)))
        .count();
    assert!(
        took >= 3,
        "the refusal must happen after handles were taken, or this measures nothing: {took} \
         page handles were requested"
    );
    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "a refused reorder left object handles alive"
    );
}

#[test]
fn a_failed_identity_read_refuses_rather_than_skipping_the_page() {
    // THE HAZARD THE MODULE HEADER NAMES, measured at last.
    //
    // `qpdf_oh_get_object_id` and `qpdf_oh_get_generation` are both
    // `do_with_oh<int>(.., return_T<int>(0), ..)`: on failure each returns 0 and latches the
    // error. So `(0, 0) == (0, 0)` reads as "this page is already in place" -- the answer that
    // means DO NOTHING -- and the page is silently left where it was, in an operation whose
    // whole invariant is that every page ends up where it was asked to be.
    //
    // Deleting the drain at `move_into_place` left all 198 tests green, because the fake
    // returned 0 for an unknown handle without latching anything. It latches now, and this is
    // the test that fails if the drain goes.
    let state = FakeHeap::new();
    // THE FAILURE LANDS ON THE LAST COMPARISON, and that placement is the whole test.
    //
    // With every `oh_object` failing, the latched error is drained by the NEXT iteration's
    // `page_handle` and the operation refuses whatever the drain does -- so the test could not
    // tell the two apart, which is exactly what happened: deleting the drain left it green
    // twice over. On the last iteration there is no next `page_handle`, so the drain is the
    // only thing between a failed identity read and a written document.
    //
    // Four pages, two `oh_object` calls each: the seventh call is the last page's.
    let script = QpdfScript {
        page_count: 4,
        oh_object_fails_after: Some(6),
        ..QpdfScript::default()
    };
    let live = Arc::clone(&script.live_handles);
    let bridge = Arc::new(FakeQpdf::new(Arc::clone(&state), script));
    let engine = WebQpdf::new(Arc::clone(&bridge) as Arc<dyn QpdfBridge>);

    let refused = reordered(&engine, 4, &[3, 2, 1, 0]);
    assert!(
        refused.is_err(),
        "a failed identity read compares EQUAL, so it must be refused rather than read as \
         'already in place'"
    );

    // IT MUST FAIL AT THE COMPARISON, AND `is_err()` CANNOT SEE THAT.
    //
    // Deleting the drain leaves this operation failing anyway: every page compares equal,
    // nothing moves, and the latched error surfaces at the drain after the write instead. So
    // the first assertion passes either way -- measured, by deleting the drain and watching
    // this test stay green.
    //
    // What separates the two is WHERE it stops. With the drain, the refusal happens before a
    // single byte is written. Without it, the operation walks every page doing nothing, then
    // writes out a document whose pages are in the ORIGINAL order and only then notices --
    // and on any engine where that latched error did not survive to the write, that document
    // would be returned as a success.
    assert!(
        !state
            .calls()
            .iter()
            .any(|c| matches!(c, Call::InitWriteMemory)),
        "the reorder went on to write a document after an identity read had failed; it must \
         refuse at the comparison, because a comparison that failed reads as 'already in \
         place' and produces a document with its pages untouched"
    );
    assert_eq!(*live.lock().expect("not poisoned"), 0);
}

#[test]
fn a_document_left_part_way_through_a_failed_reorder_will_not_be_written() {
    // A removal that succeeds with an insertion that fails leaves the document one page
    // short, and `reorder` takes `&Source` -- so nothing in the type system stops a caller
    // trying again and writing out a document that is silently missing a page. Losing a page
    // quietly is the one thing this operation's invariant forbids. Found by security review.
    let state = FakeHeap::new();
    let script = QpdfScript {
        page_count: 4,
        add_page_at_status: 2,
        ..QpdfScript::default()
    };
    let bridge = Arc::new(FakeQpdf::new(Arc::clone(&state), script));
    let engine = WebQpdf::new(Arc::clone(&bridge) as Arc<dyn QpdfBridge>);

    let source = PageReorderer::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");
    let permutation = Permutation::of(vec![3, 2, 1, 0], 4).expect("a valid permutation");

    let first = engine.reorder(&source, &permutation, &rotate_options());
    assert!(
        first.is_err(),
        "the scripted insertion failure must propagate"
    );

    // THE SECOND ATTEMPT IS THE POINT. Without the poison flag this returns `Ok` with a
    // document the fake's own page order shows is short.
    let again = engine.reorder(&source, &permutation, &rotate_options());
    assert!(
        matches!(again, Err(Error::Internal(_))),
        "a source left part-way through a failed reordering must refuse to be written"
    );
}

#[test]
fn an_order_for_a_different_document_is_refused() {
    let (engine, _bridge) = reorder_engine(4);
    let source = PageReorderer::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");
    // A valid permutation -- of three pages, for a four-page document.
    let permutation = Permutation::of(vec![2, 0, 1], 3).expect("a valid 3-permutation");
    assert!(matches!(
        engine.reorder(&source, &permutation, &rotate_options()),
        Err(Error::InvalidArgument(_))
    ));
}

/// Reading an output back must not install a second logger (ADR 0022, and the leak it nearly
/// reintroduced).
///
/// `OutputReader::fresh` returns a new engine VALUE, and the first version built it with
/// `WebQpdf::new`. That makes a fresh `installed: OnceLock`, so the witness re-entered
/// `install()` and called `logger_create()` again — a `qpdflogger_handle` plus three
/// `Pl_Discard` pipelines leaked per operation into a heap that never shrinks, which is
/// exactly the growth `qpdf.rs`'s "one logger, not one per operation" header records. Found
/// by security review, and by nothing here: the retained-allocation detector already existed
/// and no test drove the new call.
#[test]
fn a_fresh_witness_shares_the_installed_logger_rather_than_making_another() {
    let state = {
        let (engine, state) = structure_engine(QpdfScript::default());

        // One operation's worth: open a document, then read an "output" back through a fresh
        // witness, which is what every verified operation now does.
        let _source = crate::PageRotator::open(
            &engine,
            ordinary_pdf().into_boxed_slice(),
            &rotate_options(),
        )
        .expect("open");
        let witness = crate::OutputReader::fresh(&engine);
        let read = crate::OutputReader::open_output(&witness, &ordinary_pdf(), &rotate_options())
            .expect("read the output back");
        let _ = crate::OutputReader::page_count(&witness, &read).expect("page count");
        state
    };

    let loggers = state
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::LoggerCreate))
        .count();
    assert_eq!(
        loggers, 1,
        "an operation plus its verification created {loggers} loggers"
    );

    // AND THE SETTINGS ONCE. `install()` applies the global ceilings, so a second install is
    // both a leak and seven redundant FFI calls per operation.
    let installs = state
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::GlobalSet(..)))
        .count();
    assert_eq!(
        installs,
        crate::codes::qpdf::policy::settings().len(),
        "the global limits were applied more than once"
    );

    assert_eq!(
        state.retained(),
        1,
        "the witness left {} long-lived allocations behind",
        state.retained()
    );
    state.assert_empty();
}

// --- the rotation sweep, which had no web coverage at all (ADR 0022) ----------------------
//
// The sweep is the read ADR 0022's witness is built from, and until security review looked
// there was no test here that called it: the `live_handles` counter covers `rotate` and
// `reorder`, and the one `OutputReader` test stops at `page_count`. The code was correct; a
// leak or a missing checkpoint added to it would have gone unnoticed.

/// A script whose pages carry their own `/Rotate`, so the walk stops at the page.
///
/// NOT `inheriting_script()`: that one answers "dictionary" to every `/Parent`, so the climb
/// never terminates and the sweep refuses with a depth exhaustion -- correct behaviour, and
/// not what these tests are asking about.
fn own_rotation_script() -> QpdfScript {
    QpdfScript {
        page_count: 3,
        oh_int_value: 90,
        oh_type_codes: [
            ("/Rotate".to_owned(), 4), // ot_integer, found on the page itself
            ("/Parent".to_owned(), 9), // ot_dictionary, never reached
        ]
        .into_iter()
        .collect(),
        ..QpdfScript::default()
    }
}

#[test]
fn every_object_handle_the_web_rotation_sweep_takes_is_released() {
    let script = own_rotation_script();
    let live = Arc::clone(&script.live_handles);
    let (engine, _state) = structure_engine(script);

    let source = PageRotator::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");

    let clock = stopped();
    let deadline = burrow_types::Deadline::start(clock.as_ref(), &Limits::DEFAULT);
    let rotations = PageRotator::rotations(&engine, &source, &rotate_options(), &deadline)
        .expect("the sweep reads every page");
    assert_eq!(
        rotations,
        vec![90, 90, 90],
        "one value per page, read not judged"
    );

    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "the sweep left object handles alive in qpdf's cache"
    );
}

#[test]
fn a_refused_web_rotation_sweep_releases_its_handles_too() {
    // THE ERROR PATH, which is where a hand-written release is actually lost. The refusal is a
    // spent deadline, raised at the top of the loop -- so the pages before it have already
    // taken and returned handles, and the refusal itself must take none.
    let script = own_rotation_script();
    let live = Arc::clone(&script.live_handles);
    let (engine, _state) = structure_engine(script);

    let source = PageRotator::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");

    let clock = Arc::new(ManualClock::new(0));
    let limits = Limits::with(|l| l.max_duration_ms = 10);
    let deadline = burrow_types::Deadline::start(clock.as_ref(), &limits);
    clock.advance(11);

    let refused = PageRotator::rotations(
        &engine,
        &source,
        &OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>),
        &deadline,
    );
    // AND IT IS THE CALLER'S DEADLINE THAT REFUSED IT. A sweep that started its own would get
    // a full budget from a clock reading 11 and sweep all three pages happily.
    assert!(
        matches!(&refused, Err(Error::LimitExceeded { limit, .. }) if *limit == "max_duration_ms"),
        "got {refused:?}"
    );
    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "a refused sweep left object handles alive"
    );
}

// --- the web split, and the one thing the differential corpus cannot see -------------------
//
// `split`'s pruning POLICY is shared between the two implementations (`crate::prune`), so the
// corpus can no longer catch the two paths pruning differently -- there is one pruning. What is
// left is one path never reaching it, and until these tests existed nothing in `cargo test`
// called `web/extract.rs` at all: security review deleted the `prune_output` call from it and
// all 27 test binaries stayed green.
//
// These drive the web extractor against the fake, so a deletion there is a failure here.

/// A source whose first page carries four keys, three of which the allowlist does not name.
///
/// The page is a dictionary and nothing else is: `/Annots` and `/Resources` read as null, which
/// is what qpdf reports for an absent key, so the walk reaches the page-key pass with nothing
/// else to do. That is the narrowest document that can tell pruning from its absence.
fn split_script() -> QpdfScript {
    QpdfScript {
        page_count: 2,
        oh_type_codes: [("<page 0>".to_owned(), 9)].into_iter().collect(),
        oh_unparsed: [(
            "<page 0>".to_owned(),
            // `/Type` and `/Contents` are on the allowlist; `/B`, `/AA` and `/Thumb` are the
            // article-bead, additional-action and thumbnail channels ADR 0019 §2a names.
            "<< /Type /Page /Contents 3 0 R /B 4 0 R /AA 5 0 R /Thumb 6 0 R >>".to_owned(),
        )]
        .into_iter()
        .collect(),
        ..QpdfScript::default()
    }
}

fn split_source(engine: &WebQpdf) -> <WebQpdf as PageExtractor>::Source {
    PageExtractor::open(engine, ordinary_pdf().into_boxed_slice(), &rotate_options())
        .expect("the fake opens")
}

#[test]
fn the_web_extractor_reaches_the_shared_pruning_policy() {
    let script = split_script();
    let live = Arc::clone(&script.live_handles);
    let (engine, state) = structure_engine(script);
    let source = split_source(&engine);

    let part = PageExtractor::extract(&engine, &source, 0, 1, &rotate_options())
        .expect("one page comes out");
    assert_eq!(
        part,
        b"%PDF-1.7\nmerged\n".to_vec(),
        "the bytes that reach Rust are the bytes the engine wrote"
    );

    // WHAT THE POLICY DID, by name. A count would pass for three removals of the wrong keys.
    let mut removed: Vec<String> = state
        .calls()
        .iter()
        .filter_map(|c| match c {
            Call::OhRemoveKey { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect();
    removed.sort();
    assert_eq!(
        removed,
        vec!["/AA".to_owned(), "/B".to_owned(), "/Thumb".to_owned()],
        "the page-key allowlist did not run on the web path"
    );

    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "the prune left object handles alive in qpdf's cache"
    );
}

#[test]
fn the_web_split_refuses_a_layered_document_and_releases_its_handles() {
    // THE REFUSAL LIVES INSIDE THE POLICY, which is what makes the conformance corpus able to
    // see a path that skipped pruning: a path that does not prune does not refuse, it succeeds.
    // This is that refusal, reached through the web extractor.
    let mut script = split_script();
    script.oh_type_codes.extend([
        ("/Resources".to_owned(), 9),
        ("/Properties".to_owned(), 9),
        ("/MC0".to_owned(), 9),
        ("/Type".to_owned(), 7),
    ]);
    script
        .oh_unparsed
        .insert("/Properties".to_owned(), "<< /MC0 7 0 R >>".to_owned());
    // `/Type` names `/OCG`, which is what makes the entry a layer rather than an ordinary
    // marked-content property list. Refusing on the presence of `/Properties` would refuse
    // every tagged PDF, which has nothing hidden in it.
    script
        .oh_names
        .insert("/Type".to_owned(), "/OCG".to_owned());
    let live = Arc::clone(&script.live_handles);
    let (engine, state) = structure_engine(script);
    let source = split_source(&engine);

    let refused = PageExtractor::extract(&engine, &source, 0, 1, &rotate_options());
    assert!(
        matches!(&refused, Err(Error::Unsupported(message)) if message.contains("optional content")),
        "got {refused:?}"
    );

    // AND IT REFUSED BEFORE IT EDITED ANYTHING. A refusal that arrives after half the keys are
    // gone is a half-pruned document nobody receives, and a writer that ran anyway would have.
    assert!(
        !state
            .calls()
            .iter()
            .any(|c| matches!(c, Call::OhRemoveKey { .. } | Call::Write)),
        "the refusal came after the output had already been edited or written"
    );
    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "a refused split left object handles alive"
    );
}

#[test]
fn the_web_split_strips_the_leaking_keys_from_every_annotation_it_keeps() {
    // THE `/Annots` HALF OF THE POLICY, which the page-key test above does not reach: the page's
    // `/Annots` is on the allowlist, so what protects it is the per-annotation rule list.
    let mut script = split_script();
    script.oh_type_codes.extend([
        ("/Annots".to_owned(), 8),    // ot_array
        ("/Annots[0]".to_owned(), 9), // ot_dictionary
        ("/Annots[1]".to_owned(), 9),
    ]);
    script.oh_array_len.insert("/Annots".to_owned(), 2);
    let live = Arc::clone(&script.live_handles);
    let (engine, state) = structure_engine(script);
    let source = split_source(&engine);

    PageExtractor::extract(&engine, &source, 0, 1, &rotate_options()).expect("one page comes out");

    // EVERY KEY THE POLICY REMOVED, by name and without duplicates. A count would be satisfied
    // by five removals of the wrong keys, and the three annotation rules are the `/AcroForm`
    // answer ADR 0019 §2b records -- `/Parent` is the one that reaches a field's `/V`.
    let mut keys: Vec<String> = state
        .calls()
        .iter()
        .filter_map(|c| match c {
            Call::OhRemoveKey { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect();
    keys.sort();
    keys.dedup();
    assert_eq!(
        keys,
        vec![
            "/A".to_owned(),
            "/AA".to_owned(),
            "/B".to_owned(),
            "/Dest".to_owned(),
            "/Parent".to_owned(),
            "/StructParent".to_owned(),
            "/Thumb".to_owned(),
        ],
        "the annotation rules and the page allowlist did not both run"
    );

    // BACKWARDS, and this is not a style point: `qpdf_oh_erase_item` shifts everything after the
    // erased item down, so a forward loop skips the annotation that takes a removed one's place.
    // TWO passes over the array, each descending -- the filter, then the optional-content sweep.
    let visited: Vec<i32> = state
        .calls()
        .iter()
        .filter_map(|c| match c {
            Call::OhArrayItem { at, .. } => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(visited, vec![1, 0, 1, 0], "an annotation walk ran forwards");

    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "the annotation walk left object handles alive"
    );
}

#[test]
fn a_page_run_outside_the_web_source_is_refused_before_a_destination_is_opened() {
    let script = split_script();
    let (engine, state) = structure_engine(script);
    let source = split_source(&engine);
    let before = state.calls().len();

    for (first, count) in [(0, 0), (1, 2), (u64::MAX, 1)] {
        let refused = PageExtractor::extract(&engine, &source, first, count, &rotate_options());
        assert!(
            matches!(refused, Err(Error::InvalidArgument(_))),
            "({first}, {count}) was not refused: {refused:?}"
        );
    }
    assert_eq!(
        state.calls().len(),
        before,
        "a refused run still crossed the bridge"
    );
}

// ------------------------------------------------------------------ compress, on the web

/// An engine, the heap its calls are recorded on, and the live-handle counter.
///
/// Shaped like `reorder`'s setup rather than inventing an accessor: `FakeQpdf` records onto
/// the `FakeHeap` it is given, and the handle counter lives on the script.
fn compress_engine(pages: i32) -> (WebQpdf, Arc<FakeHeap>, Arc<std::sync::Mutex<i64>>) {
    let state = FakeHeap::new();
    let script = QpdfScript {
        // A `/Rotate` THE WALK CAN FIND ON THE PAGE, so the `/Parent` climb terminates. The
        // default script resolves `/Parent` forever and the sweep refuses at the depth ceiling
        // -- which is the ceiling working, and is not what these tests are about.
        oh_int_value: 90,
        oh_type_codes: [
            ("/Rotate".to_owned(), 4), // ot_integer
            ("/Parent".to_owned(), 9),
        ]
        .into_iter()
        .collect(),
        page_count: pages,
        ..QpdfScript::default()
    };
    let live = Arc::clone(&script.live_handles);
    let bridge = Arc::new(FakeQpdf::new(state.clone(), script));
    (WebQpdf::new(bridge as Arc<dyn QpdfBridge>), state, live)
}

#[test]
fn the_web_compress_sets_the_object_stream_mode_and_sets_it_to_generate() {
    // THE ONE THING THAT DISTINGUISHES THIS OPERATION, asserted as a value rather than as a
    // call. A `compress` that made the call with `qpdf_o_preserve` would produce a valid
    // document with the right page count, the right rotations and every page intact -- and
    // would be a plain write. The mode is the whole difference, so the mode is what is checked.
    let (engine, state, _live) = compress_engine(4);
    let source = DocumentCompressor::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");
    let _ = engine
        .compress(&source, &rotate_options())
        .expect("the fake compresses");

    let calls = state.calls();
    assert!(
        calls.contains(&Call::SetObjectStreamMode(2)),
        "compress did not ask for object stream generation: {calls:?}"
    );
    assert!(
        !calls.contains(&Call::SetObjectStreamMode(1)),
        "compress asked to PRESERVE object streams, which is a plain write: {calls:?}"
    );
}

#[test]
fn no_other_web_operation_asks_for_object_streams() {
    // THE NEAR-MISS for the test above. Without it that assertion is satisfied by a bridge
    // that records the call for everything, or by an operation that sets the mode by accident.
    // `rotate` goes through the same write path on the same fake.
    let (engine, state, _live) = compress_engine(4);
    let source = PageRotator::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");
    let _ = engine
        .rotate(&source, &[0], Rotation::None, &rotate_options())
        .expect("the fake rotates");

    let calls = state.calls();
    assert!(
        !calls
            .iter()
            .any(|c| matches!(c, Call::SetObjectStreamMode(_))),
        "an operation that is not compress set the object stream mode: {calls:?}"
    );
}

#[test]
fn the_web_compress_write_path_runs_in_order() {
    // THE SETTERS COME AFTER `init_write_memory`, never before: the writer does not exist
    // until that call succeeds, and both of them dereference it. Natively that ordering was
    // found by core dump (ADR 0017); here the fake records the sequence so it cannot regress
    // silently.
    let (engine, state, _live) = compress_engine(4);
    let source = DocumentCompressor::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");
    let _ = engine
        .compress(&source, &rotate_options())
        .expect("the fake compresses");

    let calls = state.calls();
    let write_path: Vec<&Call> = calls
        .iter()
        .filter(|c| {
            matches!(
                c,
                Call::InitWriteMemory
                    | Call::SetDeterministicId(_)
                    | Call::SetObjectStreamMode(_)
                    | Call::Write
                    | Call::GetBufferLength
                    | Call::GetBuffer
                    | Call::CopyOut(_)
            )
        })
        .collect();
    assert_eq!(
        write_path,
        vec![
            &Call::InitWriteMemory,
            &Call::SetDeterministicId(true),
            &Call::SetObjectStreamMode(2),
            &Call::Write,
            &Call::GetBufferLength,
            &Call::GetBuffer,
            &Call::CopyOut(16),
        ],
        "the compress write path ran out of order: {calls:?}"
    );
}

#[test]
fn every_object_handle_the_web_compress_takes_is_released() {
    // The rotation sweep takes one handle per page. Across the bridge a handle is a `u32`
    // with nothing to attach a `Drop` to, so release is a discipline rather than a type --
    // and qpdf's cache only grows, so a leak is invisible except to a count.
    let (engine, state, live) = compress_engine(6);
    let source = DocumentCompressor::open(
        &engine,
        ordinary_pdf().into_boxed_slice(),
        &rotate_options(),
    )
    .expect("the fake opens");

    let deadline = burrow_types::Deadline::start(
        rotate_options().clock.as_ref(),
        &burrow_types::Limits::DEFAULT,
    );
    let _ = DocumentCompressor::rotations(&engine, &source, &rotate_options(), &deadline)
        .expect("the sweep runs");
    let _ = engine
        .compress(&source, &rotate_options())
        .expect("the fake compresses");

    assert_eq!(
        *live.lock().expect("not poisoned"),
        0,
        "the web compression left object handles alive: {:?}",
        state.calls()
    );
}

#[test]
fn the_web_and_native_object_stream_modes_agree() {
    // TWO CONSTANTS, ONE VALUE. `web/compress.rs` spells `qpdf_o_generate` itself because the
    // native `ffi` module is gated on the engines being linked and does not exist on that
    // path. A divergence here would be `compress` silently not compressing on the web, with
    // every page count and every rotation still correct -- which no differential case could
    // see, because the corpus compares outcomes and neither would be an error.
    //
    // The native constant is only compiled when the engines are, so this asserts against the
    // value from `Constants.h:134-138` directly rather than importing it.
    assert_eq!(
        super::compress::QPDF_O_GENERATE,
        2,
        "qpdf_o_generate is 2 in Constants.h:134-138"
    );
}

// ---------------------------------------------------------------------------------
// Rendering on the web path (#57, ADR 0027).
//
// The native tests drive real PDFium and prove the pixels are right. These prove the
// things a browser will not reproduce on demand: that the page handle and the bitmap are
// released on EVERY exit, that the stride is asked for rather than assumed, and that
// `max_pixels` fires before anything is allocated.
// ---------------------------------------------------------------------------------

use crate::{PageRenderer, Raster};

fn open_for_render(
    script: PdfiumScript,
    limits: Limits,
) -> (WebPdfium, Arc<FakeHeap>, super::pdfium::WebDocument) {
    let (engine, state) = document_engine(script);
    let doc = engine
        .open(
            ordinary_pdf().into_boxed_slice(),
            &OpenOptions::new(limits, stopped()),
        )
        .expect("an ordinary document should open");
    (engine, state, doc)
}

fn render(
    engine: &WebPdfium,
    doc: &super::pdfium::WebDocument,
    width: u32,
    height: u32,
    limits: Limits,
) -> Result<Raster> {
    let options = OpenOptions::new(limits, stopped());
    let deadline = burrow_types::Deadline::start(options.clock.as_ref(), &options.limits);
    PageRenderer::render(engine, doc, 0, width, height, &options, &deadline)
}

#[test]
fn a_web_render_returns_rgba_at_the_size_that_was_asked_for_and_leaks_nothing() {
    let (engine, state, doc) = open_for_render(PdfiumScript::default(), Limits::default());
    let raster = render(&engine, &doc, 4, 4, Limits::default()).expect("a page should render");

    assert_eq!((raster.width, raster.height), (4, 4));
    assert_eq!(raster.rgba.len(), 4 * 4 * 4);
    // The fake inks the top-left quadrant and the fill made the rest opaque white. Both
    // halves matter: the ink proves the render ran, the white proves the fill did.
    assert_eq!(&raster.rgba[0..4], &[0, 0, 0, 255]);
    assert_eq!(&raster.rgba[12..16], &[255, 255, 255, 255]);

    drop(doc);
    // The page handle AND the bitmap. Neither has a `Drop` on the other side of a real
    // bridge, so a missing release is a worker that grows until it is recycled.
    state.assert_empty();
}

#[test]
fn a_padded_stride_is_read_from_the_engine_rather_than_computed_from_the_width() {
    // Four bytes of padding per row. A reader that assumed `width * 4` would walk into the
    // padding on every row after the first and return a picture sheared by one pixel a row
    // -- correct on every unpadded bitmap, which is most of them.
    let (engine, state, doc) = open_for_render(
        PdfiumScript {
            stride_padding: 4,
            ..PdfiumScript::default()
        },
        Limits::default(),
    );
    let raster = render(&engine, &doc, 4, 4, Limits::default()).expect("a page should render");

    assert_eq!(raster.rgba.len(), 4 * 4 * 4);

    // THESE FOUR PROBES WERE NOT ENOUGH, AND THAT WAS MEASURED. The test originally checked
    // row 0 and row 2 only, and code review replaced `bitmap_stride(bitmap)` with `width * 4`
    // -- the exact assumption this test is named after -- and it PASSED: with a stride of 20
    // and a row of 16, the misread row 0 is still ink and the misread row 2 is still white.
    //
    // What the mutation actually produces is the fake's pre-fill leaking into the picture, so
    // that is what is asserted. 0xCD is not a colour anything here writes: the fake fills a
    // fresh bitmap with it precisely so an unfilled or misread region is visible rather than
    // plausible, and until now nothing read it.
    assert!(
        !raster.rgba.contains(&0xCD),
        "the fake's uninitialised filler reached the picture, so the rows were misread"
    );
    assert_eq!(&raster.rgba[0..4], &[0, 0, 0, 255], "row 0 is inked");
    // ROW 1, which the four-corner version skipped. Under the `width * 4` mutation this is
    // where the shear first lands: the read walks into row 0's padding.
    let row1 = 4 * 4;
    assert_eq!(
        &raster.rgba[row1..row1 + 4],
        &[0, 0, 0, 255],
        "row 1 is inked too"
    );
    // Row 2 is below the inked quadrant: opaque white, not the 0xCD the fake pre-fills with
    // and not a byte of padding.
    let row2 = 2 * 4 * 4;
    assert_eq!(&raster.rgba[row2..row2 + 4], &[255, 255, 255, 255]);

    drop(doc);
    state.assert_empty();
}

#[test]
fn the_web_render_refuses_max_pixels_before_it_asks_for_a_page() {
    let (engine, state, doc) = open_for_render(PdfiumScript::default(), Limits::default());
    let tight = Limits::with(|l| l.max_pixels = 15);

    match render(&engine, &doc, 4, 4, tight).expect_err("one pixel past the ceiling") {
        Error::LimitExceeded {
            limit,
            stage,
            requested,
            allowed,
        } => {
            assert_eq!(
                (limit, stage, requested, allowed),
                ("max_pixels", Stage::Pixels, 16, 15)
            );
        }
        other => panic!("expected LimitExceeded at Stage::Pixels, got {other:?}"),
    }

    // BEFORE ANYTHING WAS ALLOCATED, which is the whole value of the check -- so the bridge
    // was never asked for a page and never asked for a bitmap.
    let calls = state.calls();
    assert!(
        !calls.iter().any(|c| matches!(c, Call::LoadPage(_))),
        "the ceiling fired after the page was loaded: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| matches!(c, Call::BitmapCreate { .. })),
        "the ceiling fired after the bitmap was allocated: {calls:?}"
    );

    drop(doc);
    state.assert_empty();
}

#[test]
fn a_bitmap_that_could_not_be_allocated_is_io_and_not_a_limit() {
    // The two are different failures and must not be reported alike: a ceiling is the
    // caller's request being refused, an allocation failure is the engine running out. A
    // null return read as the ceiling would mean the ceiling only ever fired after the cost
    // had already been paid.
    let (engine, state, doc) = open_for_render(
        PdfiumScript {
            bitmap_alloc_succeeds: false,
            ..PdfiumScript::default()
        },
        Limits::default(),
    );
    assert!(matches!(
        render(&engine, &doc, 4, 4, Limits::default()),
        Err(Error::Io(_))
    ));

    // The page was loaded and must still have been closed on the way out.
    drop(doc);
    state.assert_empty();
}

#[test]
fn a_page_that_will_not_load_is_malformed_and_allocates_no_bitmap() {
    let (engine, state, doc) = open_for_render(
        PdfiumScript {
            page_load_succeeds: false,
            ..PdfiumScript::default()
        },
        Limits::default(),
    );
    assert!(matches!(
        render(&engine, &doc, 4, 4, Limits::default()),
        Err(Error::Malformed(_))
    ));
    assert!(
        !state
            .calls()
            .iter()
            .any(|c| matches!(c, Call::BitmapCreate { .. }))
    );

    drop(doc);
    state.assert_empty();
}

#[test]
fn a_buffer_the_engine_will_not_hand_over_still_destroys_the_bitmap() {
    let (engine, state, doc) = open_for_render(
        PdfiumScript {
            bitmap_buffer_is_null: true,
            ..PdfiumScript::default()
        },
        Limits::default(),
    );
    assert!(matches!(
        render(&engine, &doc, 4, 4, Limits::default()),
        Err(Error::Internal(_))
    ));

    let calls = state.calls();
    assert!(calls.iter().any(|c| matches!(c, Call::BitmapDestroy(_))));
    assert!(calls.iter().any(|c| matches!(c, Call::ClosePage(_))));

    drop(doc);
    state.assert_empty();
}

#[test]
fn a_stride_narrower_than_a_row_is_refused_rather_than_read_past() {
    let (engine, state, doc) = open_for_render(
        PdfiumScript {
            stride_override: Some(4),
            ..PdfiumScript::default()
        },
        Limits::default(),
    );
    assert!(matches!(
        render(&engine, &doc, 4, 4, Limits::default()),
        Err(Error::Internal(_))
    ));

    drop(doc);
    state.assert_empty();
}

#[test]
fn a_negative_stride_is_the_engine_contradicting_itself() {
    let (engine, state, doc) = open_for_render(
        PdfiumScript {
            stride_override: Some(-1),
            ..PdfiumScript::default()
        },
        Limits::default(),
    );
    assert!(matches!(
        render(&engine, &doc, 4, 4, Limits::default()),
        Err(Error::Internal(_))
    ));

    drop(doc);
    state.assert_empty();
}

#[test]
fn the_fill_happens_before_the_render_and_the_rotation_is_never_doubled() {
    // Two orderings that are silent when wrong. A fill AFTER the render erases the page and
    // returns a blank picture; a non-zero `rotate` turns a page whose `/Rotate` PDFium has
    // already applied, which is precisely the case `/rotate-pdf` exists to show.
    let (engine, state, doc) = open_for_render(PdfiumScript::default(), Limits::default());
    render(&engine, &doc, 4, 4, Limits::default()).expect("a page should render");

    let calls = state.calls();
    let fill = calls
        .iter()
        .position(|c| matches!(c, Call::BitmapFillRect { .. }))
        .expect("the bitmap should be filled");
    let drawn = calls
        .iter()
        .position(|c| matches!(c, Call::RenderPageBitmap { .. }))
        .expect("the page should be drawn");
    assert!(fill < drawn, "the fill must precede the render: {calls:?}");

    match calls
        .iter()
        .find(|c| matches!(c, Call::RenderPageBitmap { .. }))
    {
        Some(Call::RenderPageBitmap { rotate, flags, .. }) => {
            assert_eq!(*rotate, 0, "PDFium applies /Rotate itself");
            assert_eq!(*flags, 0, "no FPDF_ANNOT; see the constant");
        }
        other => panic!("expected a render call, got {other:?}"),
    }

    drop(doc);
    state.assert_empty();
}

#[test]
fn a_page_past_the_end_never_reaches_the_bridge_on_the_web_path_either() {
    let (engine, state, doc) = open_for_render(PdfiumScript::default(), Limits::default());
    let options = OpenOptions::new(Limits::default(), stopped());
    let deadline = burrow_types::Deadline::start(options.clock.as_ref(), &options.limits);
    assert!(matches!(
        PageRenderer::render(&engine, &doc, 1, 4, 4, &options, &deadline),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        PageRenderer::page_size(&engine, &doc, 1),
        Err(Error::InvalidArgument(_))
    ));
    assert!(
        !state.calls().iter().any(|c| matches!(c, Call::LoadPage(_))),
        "an out-of-range index must not reach the engine"
    );

    drop(doc);
    state.assert_empty();
}

#[test]
fn page_size_closes_the_page_it_opened() {
    let (engine, state, doc) = open_for_render(PdfiumScript::default(), Limits::default());
    let (width, height) =
        PageRenderer::page_size(&engine, &doc, 0).expect("a page should report its size");
    assert_eq!((width, height), (200.0, 400.0));

    drop(doc);
    state.assert_empty();
}
