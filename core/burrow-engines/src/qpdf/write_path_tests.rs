//! The write verb, against real qpdf rather than a fake.
//!
//! `web/tests.rs` drives `Session::replace_stream_data` through `FakeQpdf`, which is the right
//! place to provoke a latched error and a refused allocation — a fake is the only way to make
//! those happen on demand. What a fake cannot answer is whether **qpdf** does what the wrapper
//! says it does: that it copies the buffer before returning, that a null object really means
//! "no filter", and that the replaced bytes survive a write and a reopen.
//!
//! So these run on the engine. They are `#[cfg(all(test, feature = "native-engines"))]`, like
//! every other test in this module that needs a document.

use std::sync::Arc;

use burrow_types::{Clock, Limits, SystemClock};

use super::handle::ObjectHandle;
use super::{Document, open_document};
use crate::OpenOptions;
use crate::minimal_pdf;

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(SystemClock::new()) as Arc<dyn Clock>,
    )
}

/// Open a one-page document and hand back its page's `/Contents` stream.
fn page_contents(document: &Document) -> ObjectHandle<'_> {
    // SAFETY: the document opened successfully and page 0 is below its page count.
    let page = unsafe { ObjectHandle::page(document, 0) };
    page.key(c"/Contents".as_ptr())
}

#[test]
fn qpdf_copies_the_buffer_before_returning_so_the_bytes_may_be_dropped() {
    // THE ASSUMPTION THE WHOLE `finally { _free(buf) }` SHAPE RESTS ON, and the one a qpdf
    // version bump could invalidate. `qpdf-c.h:942-944` states it; this measures it, by
    // handing qpdf a buffer that is dropped immediately afterwards and then reading the
    // stream back.
    let bytes = minimal_pdf::pdf_with_ink();
    let (document, _, _, _) = open_document(bytes.into(), &options()).expect("opens");
    let contents = page_contents(&document);
    let null = ObjectHandle::new_null(&document);

    {
        let scratch = b"BT /F1 12 Tf (replaced) Tj ET".to_vec();
        contents
            .replace_stream_data(&scratch, &null, &null)
            .expect("the replace succeeds");
        drop(scratch);
    }

    let read_back = contents
        .stream_data()
        .expect("the stream reads back")
        .expect("and it decoded");
    assert_eq!(
        read_back.as_slice(),
        b"BT /F1 12 Tf (replaced) Tj ET",
        "qpdf did not copy the buffer, or did not keep what it copied"
    );
}

#[test]
fn a_null_filter_leaves_the_stream_uncompressed_and_readable() {
    // The other half of the C API's shape: `filter` and `decode_parms` are OBJECTS, and a null
    // object is how "no filter" is said. If a null meant "leave the old filter alone", the new
    // raw bytes would be read back through the previous `/FlateDecode` and come out as noise.
    let bytes = minimal_pdf::pdf_with_ink();
    let (document, _, _, _) = open_document(bytes.into(), &options()).expect("opens");
    let contents = page_contents(&document);
    let null = ObjectHandle::new_null(&document);

    contents
        .replace_stream_data(b"0 0 1 rg", &null, &null)
        .expect("the replace succeeds");

    let dictionary = contents.stream_dict();
    let filter = dictionary.key(c"/Filter".as_ptr());
    // `qpdf_ot_null` is 2 (`qpdf-c.h`'s `qpdf_object_type_e`: uninitialized, reserved, null,
    // …) — the first draft of this test guessed 1 and the engine said 2, which is the reason
    // to assert against the engine rather than against a remembered enum. The point is only
    // that it is not an array, and not a name naming a decoder.
    assert_eq!(
        filter.type_code(),
        2,
        "a null filter did not clear the stream's /Filter"
    );
}

#[test]
fn the_replaced_bytes_survive_a_write_and_a_reopen() {
    // END TO END, because a replace that only holds in memory is not a redaction. The written
    // document is reopened through the same path an operation's output would be.
    let bytes = minimal_pdf::pdf_with_ink();
    let (document, _, _, _) = open_document(bytes.into(), &options()).expect("opens");
    {
        let contents = page_contents(&document);
        let null = ObjectHandle::new_null(&document);
        contents
            .replace_stream_data(b"(kept-after-write) Tj", &null, &null)
            .expect("the replace succeeds");
    }

    let written = super::extract::write_out(
        &document,
        &document,
        super::extract::ObjectStreams::Preserve,
    )
    .expect("the document writes");
    let (reopened, _, _, _) = open_document(written.into(), &options()).expect("reopens");
    let after = page_contents(&reopened)
        .stream_data()
        .expect("reads")
        .expect("decodes");

    assert_eq!(after.as_slice(), b"(kept-after-write) Tj");
}

#[test]
fn a_filter_handle_from_another_document_is_refused_rather_than_written() {
    // THE LEAK SHAPE SECURITY REVIEW FOUND, as a test. qpdf handles are bare `++next_oh`
    // counters, so a handle from another document does not fail -- it COLLIDES with a live
    // handle here and is written as `/Filter`, with no error. The wrapper checks the document
    // rather than trusting the handle, which is `core/CLAUDE.md`'s rule one level up.
    let first = minimal_pdf::pdf_with_ink();
    let second = minimal_pdf::pdf_with_ink();
    let (a, _, _, _) = open_document(first.into(), &options()).expect("opens");
    let (b, _, _, _) = open_document(second.into(), &options()).expect("opens");

    let contents = page_contents(&a);
    let foreign = ObjectHandle::new_null(&b);

    let refused = contents.replace_stream_data(b"0 0 1 rg", &foreign, &foreign);

    assert!(
        refused.is_err(),
        "a filter handle from another document was accepted"
    );
    // AND NOTHING WAS WRITTEN. Without this the test passes on a wrapper that refuses after
    // the engine call, which is a refusal reported over a document already modified.
    let unchanged = contents.stream_data().expect("reads").expect("decodes");
    assert_ne!(unchanged.as_slice(), b"0 0 1 rg");
}
