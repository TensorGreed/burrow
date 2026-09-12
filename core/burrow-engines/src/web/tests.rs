//! The web orchestration, driven against a fake bridge on an ordinary host.
//!
//! These are the tests that a `cfg(target_arch = "wasm32")` module could not have had.
//! They assert the things Playwright cannot see from outside a worker — that the buffer is
//! freed on every failure path, that the password copy in the engine heap is wiped, that
//! the page limit is checked before a document escapes, and that the checks happen in the
//! order the native path uses.

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock, Password, Stage};

use super::fake::{Call, FakeHeap, FakePdfium, FakeQpdf, PdfiumScript, QpdfScript};
use super::{WebPdfium, WebQpdf};
use crate::{CheckOptions, DocumentEngine, OpenOptions, StructureEngine};

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
    assert_eq!(qpdf.name(), "qpdf-wasm");
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

/// The web qpdf path does **not** apply the PDFium-derived size estimate, because the
/// native qpdf path does not either.
///
/// An earlier version called `estimate::check_open_memory` here. The two paths then
/// disagreed on any file between `max_memory_bytes / 1.25` and `max_memory_bytes` — a
/// divergence in the module whose docs claim the two cannot diverge, and exactly what
/// ROADMAP item 12's differential harness exists to catch.
#[test]
fn a_file_under_the_memory_ceiling_is_checked_rather_than_estimated_away() {
    let bytes = ordinary_pdf();
    // A ceiling above the input but below `input + input/4 + overhead`, which is what the
    // estimate would have compared against.
    let limits = Limits::with(|l| l.max_memory_bytes = u64::try_from(bytes.len()).unwrap() + 16);

    let (engine, state) = structure_engine(QpdfScript {
        page_count: 2,
        ..QpdfScript::default()
    });
    let report = engine
        .check(
            bytes.into_boxed_slice(),
            &CheckOptions::new(limits, stopped()),
        )
        .expect("a structural check must not apply PDFium's open-cost estimate");
    assert_eq!(report.pages, 2);
    state.assert_empty();
}

/// The measured memory check runs on the qpdf path too.
///
/// `QpdfBridge::heap_bytes` was declared and called from nowhere — a method that existed to
/// make the two bridges look symmetric, in a trait whose own docs describe the method list
/// as the audit surface. Unlike the size estimate (which is PDFium-derived and deliberately
/// absent here), this one reads what the engine actually did.
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
