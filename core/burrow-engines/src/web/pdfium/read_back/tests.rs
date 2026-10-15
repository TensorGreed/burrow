//! The web second reading against a fake bridge: what it reads, and that it closes what it opens
//! on every exit, in the order native does.

use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Limits, ManualClock};

use super::WebPdfiumReading;
use crate::OpenOptions;
use crate::redact_read_back::{MAX_CHARS, PlacedChar, SecondReading};
use crate::web::WebPdfium;
use crate::web::fake::{Call, FakeChar, FakeHeap, FakePdfium, PdfiumScript};

fn reading(text: Option<Vec<FakeChar>>) -> (WebPdfium, Arc<FakeHeap>) {
    let state = FakeHeap::new();
    let bridge = FakePdfium::new(Arc::clone(&state), PdfiumScript::default()).with_text(text);
    (WebPdfium::new(Arc::new(bridge)), state)
}

fn options() -> OpenOptions<'static> {
    OpenOptions::new(Limits::default(), Arc::new(ManualClock::new(0)))
}

/// The request's one deadline, started now on `options`' clock.
fn deadline(options: &OpenOptions<'_>) -> Deadline {
    Deadline::start(options.clock.as_ref(), &options.limits)
}

fn pdf() -> Vec<u8> {
    crate::minimal_pdf::pdf_with_pages(1)
}

fn ch(x: f64, y: f64, generated: i32) -> FakeChar {
    FakeChar {
        origin: Some((x, y)),
        generated,
    }
}

/// The text layer is closed, then the page, after every text read, success or not.
fn closes_text_before_page(calls: &[Call]) -> bool {
    let text = calls
        .iter()
        .position(|c| matches!(c, Call::TextClosePage(_)));
    let page = calls.iter().position(|c| matches!(c, Call::ClosePage(_)));
    matches!((text, page), (Some(t), Some(p)) if t < p)
}

#[test]
fn an_honest_page_is_read_as_the_engine_places_it() {
    let (engine, state) = reading(Some(vec![ch(10.0, 20.0, 0), ch(30.0, 20.0, 1)]));
    let opts = options();
    let r = WebPdfiumReading::over(&engine, &opts, deadline(&opts));
    let page = r.read_page(&pdf(), 0).expect("read").expect("an answer");
    assert_eq!(page.pages, 1);
    assert_eq!(page.page, 0);
    assert_eq!(page.size, (200.0, 400.0));
    assert_eq!(
        page.chars,
        vec![
            PlacedChar {
                origin: (10.0, 20.0),
                generated: false
            },
            PlacedChar {
                origin: (30.0, 20.0),
                generated: true
            },
        ]
    );
    assert_eq!(r.page_count(&pdf()).expect("count"), Some(1));
    assert!(closes_text_before_page(&state.calls()));
    state.assert_empty();
}

#[test]
fn only_one_means_generated_as_on_native() {
    // `FPDFText_IsGenerated` is 1, 0, or -1. Native reads `== 1` as generated and anything else
    // non-negative as not; a value the web read differently would make the platforms disagree.
    let (engine, state) = reading(Some(vec![ch(1.0, 1.0, 2)]));
    let opts = options();
    let page = WebPdfiumReading::over(&engine, &opts, deadline(&opts))
        .read_page(&pdf(), 0)
        .expect("read")
        .expect("an answer");
    assert!(!page.chars[0].generated);
    state.assert_empty();
}

#[test]
fn every_engine_failure_is_no_answer_and_closes_what_was_opened() {
    let failing: [(&str, Option<Vec<FakeChar>>); 3] = [
        ("no text layer", None),
        (
            "no origin",
            Some(vec![FakeChar {
                origin: None,
                generated: 0,
            }]),
        ),
        ("generated -1", Some(vec![ch(1.0, 1.0, -1)])),
    ];
    for (what, text) in failing {
        let (engine, state) = reading(text);
        let opts = options();
        let result = WebPdfiumReading::over(&engine, &opts, deadline(&opts)).read_page(&pdf(), 0);
        assert!(
            matches!(result, Err(Error::Malformed(_))),
            "{what}: {result:?}"
        );
        let calls = state.calls();
        assert!(
            calls.iter().any(|c| matches!(c, Call::ClosePage(_))),
            "{what}: the page was left open"
        );
        if what != "no text layer" {
            assert!(closes_text_before_page(&calls), "{what}: close order");
        }
        state.assert_empty();
    }
}

#[test]
fn a_page_past_the_character_ceiling_is_refused_before_any_is_read() {
    let many = vec![ch(1.0, 1.0, 0); MAX_CHARS + 1];
    let (engine, state) = reading(Some(many));
    let opts = options();
    let result = WebPdfiumReading::over(&engine, &opts, deadline(&opts)).read_page(&pdf(), 0);
    assert!(matches!(result, Err(Error::Unsupported(_))), "{result:?}");
    assert!(
        !state
            .calls()
            .iter()
            .any(|c| matches!(c, Call::TextCharOrigin(_))),
        "no character may be read past the ceiling"
    );
    assert!(closes_text_before_page(&state.calls()));
    state.assert_empty();
}

#[test]
fn a_page_outside_the_document_is_refused_before_it_is_loaded() {
    let (engine, state) = reading(Some(vec![]));
    let opts = options();
    let result = WebPdfiumReading::over(&engine, &opts, deadline(&opts)).read_page(&pdf(), 5);
    assert!(result.is_err(), "{result:?}");
    assert!(
        !state.calls().iter().any(|c| matches!(c, Call::LoadPage(_))),
        "a page outside the document was loaded"
    );
    state.assert_empty();
}

#[test]
fn both_opens_spend_the_one_deadline_the_request_started() {
    // THE BUDGET IS SPENT ONCE (#206's web half, design review M1). A fresh deadline per open
    // handed a reading its budget twice: 100 ms, then 150 ms between the page count and the page,
    // read Ok where native refuses. One deadline, started when the request arrived, refuses.
    let (engine, state) = reading(Some(vec![ch(1.0, 1.0, 0)]));
    let clock = Arc::new(ManualClock::new(0));
    let mut limits = Limits::default();
    limits.max_duration_ms = 100;
    let opts = OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>);
    let r = WebPdfiumReading::over(&engine, &opts, deadline(&opts));
    assert_eq!(r.page_count(&pdf()).expect("within the budget"), Some(1));
    clock.advance(150);
    let result = r.read_page(&pdf(), 0);
    assert!(
        matches!(
            result,
            Err(Error::LimitExceeded {
                limit: "max_duration_ms",
                ..
            })
        ),
        "{result:?}"
    );
    state.assert_empty();
}
