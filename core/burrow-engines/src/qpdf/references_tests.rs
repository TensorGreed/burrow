//! A reference qpdf resolves to null is refused, on both engines (#227).
//!
//! Every document here is built by hand, with the generations written into it, because the case
//! is a disagreement about one generation number and a producer would never write it. The page
//! is always the same: 300 x 400, `SECRET` at (20, 350) and `KEEP` at (20, 50) in Helvetica, and
//! the region is the top band `SECRET` sits in.
//!
//! The binding is asked directly first -- `4 1 R` over a file holding only `4 0` -- so the rule is
//! tied to what qpdf answers through the C API on each engine, not to the CLI's behaviour.

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_types::{Clock, Limits, ManualClock};

use super::web_differential_tests::NativeBridge;
use crate::codes::qpdf::object_type;
use crate::pdfsyntax::region::Region;
use crate::redact::graph::{OpensForRedaction, PdfDocument, PdfObject};
use crate::{OpenOptions, PageRedactor};

/// One object as written: its number, its generation, and its body.
type Written = (u32, u16, String);

/// A file with a classic cross-reference table, each object at the generation it is written with.
///
/// Numbers missing between 1 and the largest are free entries.
fn classic(objects: &[Written]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut entries = std::collections::BTreeMap::new();
    for (number, generation, body) in objects {
        entries.insert(*number, (out.len(), *generation));
        out.extend_from_slice(format!("{number} {generation} obj\n{body}\nendobj\n").as_bytes());
    }
    let size = entries.keys().max().map_or(1, |largest| largest + 1);
    let xref_at = out.len();
    out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for number in 1..size {
        match entries.get(&number) {
            Some((offset, generation)) => {
                out.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
            }
            None => out.extend_from_slice(b"0000000000 00001 f \n"),
        }
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n")
            .as_bytes(),
    );
    out
}

/// A file whose `members` live in one uncompressed object stream, numbered `stream` and written,
/// headed and cross-referenced at `stream_generation`, and whose cross-reference is an
/// uncompressed stream. `direct` are written in the file body.
fn with_object_stream(
    direct: &[Written],
    stream: u32,
    stream_generation: u16,
    members: &[(u32, String)],
) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    // (type, field 2, field 3) per object number, as a cross-reference stream records them.
    let mut entries: std::collections::BTreeMap<u32, (u8, u32, u16)> =
        std::collections::BTreeMap::new();
    for (number, generation, body) in direct {
        entries.insert(*number, (1, u32::try_from(out.len()).unwrap(), *generation));
        out.extend_from_slice(format!("{number} {generation} obj\n{body}\nendobj\n").as_bytes());
    }
    let mut header = String::new();
    let mut bodies = String::new();
    for (index, (number, body)) in members.iter().enumerate() {
        header.push_str(&format!("{number} {} ", bodies.len()));
        bodies.push_str(body);
        bodies.push('\n');
        entries.insert(*number, (2, stream, u16::try_from(index).unwrap()));
    }
    let data = format!("{header}{bodies}");
    entries.insert(
        stream,
        (1, u32::try_from(out.len()).unwrap(), stream_generation),
    );
    out.extend_from_slice(
        format!(
            "{stream} {stream_generation} obj\n<< /Type /ObjStm /N {} /First {} /Length {} >>\nstream\n{data}\nendstream\nendobj\n",
            members.len(),
            header.len(),
            data.len() + 1
        )
        .as_bytes(),
    );
    let xref_number = entries.keys().max().map_or(1, |largest| largest + 1);
    let xref_at = out.len();
    entries.insert(xref_number, (1, u32::try_from(xref_at).unwrap(), 0));
    let size = xref_number + 1;
    let mut table = Vec::new();
    for number in 0..size {
        let (kind, two, three) = entries.get(&number).copied().unwrap_or((0, 0, 0));
        table.push(kind);
        table.extend_from_slice(&two.to_be_bytes());
        table.extend_from_slice(&three.to_be_bytes());
    }
    out.extend_from_slice(
        format!(
            "{xref_number} 0 obj\n<< /Type /XRef /Size {size} /W [1 4 2] /Root 1 0 R /Length {} >>\nstream\n",
            table.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&table);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref_at}\n%%EOF\n").as_bytes());
    out
}

const CONTENT: &str = "BT /F1 12 Tf 20 350 Td (SECRET) Tj ET\nBT /F1 12 Tf 20 50 Td (KEEP) Tj ET\n";

/// The catalog, the page tree, the content and the font, as objects 1, 2, 4 and 5, with the page
/// as object 3 carrying `page_extra`.
fn page_objects(page_extra: &str) -> Vec<Written> {
    vec![
        (1, 0, "<< /Type /Catalog /Pages 2 0 R >>".to_owned()),
        (2, 0, "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned()),
        (3, 0, page(page_extra)),
        (
            4,
            0,
            format!(
                "<< /Length {} >>\nstream\n{CONTENT}endstream",
                CONTENT.len()
            ),
        ),
        (
            5,
            0,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_owned(),
        ),
    ]
}

fn page(extra: &str) -> String {
    format!(
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /MediaBox [0 0 300 400] \
         /Resources << /Font << /F1 5 0 R >> >> {extra} >>"
    )
}

/// The reviewer's reproduction (#227): `/Rotate` written as `6 <rotate_generation> R`, and object
/// 6 holding `90` at `object_generation`.
fn rotated_by(rotate_generation: u16, object_generation: u16) -> Vec<u8> {
    let mut objects = page_objects(&format!("/Rotate 6 {rotate_generation} R"));
    objects.push((6, object_generation, "90".to_owned()));
    classic(&objects)
}

/// The band `SECRET` sits in, measured from the top of the page as shown. On the page turned a
/// quarter, `SECRET` runs down the right-hand edge, so this band holds none of it -- which is why
/// a redaction that cannot see the turn is a leak.
const UPPER_BAND: Region = Region {
    left: 0.0,
    top: 30.0,
    width: 300.0,
    height: 40.0,
};

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

/// Redact the upper band of page 0 natively and on the web, over the same qpdf.
fn on_both(bytes: &[u8]) -> [(&'static str, burrow_types::Result<Vec<u8>>); 2] {
    let covered = BTreeSet::from([0]);
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    [
        (
            "natively",
            super::Qpdf
                .redact_page(bytes, 0, &covered, UPPER_BAND, &options())
                .map(|(output, _)| output),
        ),
        (
            "on the web",
            web.redact_page(bytes, 0, &covered, UPPER_BAND, &options())
                .map(|(output, _)| output),
        ),
    ]
}

fn refused_on_both(bytes: &[u8], rule: &str, what: &str) {
    for (engine, outcome) in on_both(bytes) {
        match outcome {
            Err(error) => assert!(
                format!("{error:?}").contains(&format!("[{rule}]")),
                "{what}, {engine}: refused, but not by [{rule}]: {error:?}"
            ),
            Ok(_) => panic!("{what}, {engine}: redacted, where [{rule}] must refuse"),
        }
    }
}

fn redacts_on_both(bytes: &[u8], what: &str) -> Vec<u8> {
    let [(_, natively), (_, on_the_web)] = on_both(bytes);
    let natively = natively.unwrap_or_else(|error| panic!("{what}, natively: {error:?}"));
    let on_the_web = on_the_web.unwrap_or_else(|error| panic!("{what}, on the web: {error:?}"));
    assert_eq!(
        natively, on_the_web,
        "{what}: the two engines wrote different bytes"
    );
    natively
}

/// PDFium's displayed size for page 0: the box, turned by whatever `/Rotate` it read.
fn pdfium_size(bytes: &[u8]) -> (f64, f64) {
    super::Qpdf
        .renderer_page_size(bytes, 0, &options())
        .expect("PDFium opens the page")
        .expect("natively there is a renderer")
}

#[test]
fn the_binding_resolves_the_generation_written_and_never_another() {
    // ASKED THROUGH THE C API, on each engine, not the CLI. Object 4 is `90` at generation 0
    // only: `(4, 1)` must be null, and `(4, 0)` the integer -- a binding that asked `(N, 0)`
    // whatever it was given would answer both alike.
    let mut objects = page_objects("");
    objects.push((6, 0, "<< /K 4 1 R >>".to_owned()));
    objects[3] = (4, 0, "90".to_owned());
    let bytes = classic(&objects);

    let native = super::Qpdf
        .open_for_redaction(&bytes, &options())
        .expect("opens")
        .0;
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    let redactor = crate::web::redact_testing::redactor(&web);
    let on_the_web = redactor
        .open_for_redaction(&bytes, &options())
        .expect("opens on the web")
        .0;
    for (engine, wrong, right, absent) in [
        (
            "natively",
            native.object(4, 1).unwrap().type_code(),
            native.object(4, 0).unwrap().type_code(),
            native.object(99, 0).unwrap().type_code(),
        ),
        (
            "on the web",
            on_the_web.object(4, 1).unwrap().type_code(),
            on_the_web.object(4, 0).unwrap().type_code(),
            on_the_web.object(99, 0).unwrap().type_code(),
        ),
    ] {
        assert_eq!(wrong, object_type::NULL, "(4, 1) over a 4 0, {engine}");
        assert_eq!(right, object_type::INTEGER, "(4, 0), {engine}");
        assert_eq!(absent, object_type::NULL, "an absent number, {engine}");
    }
}

#[test]
fn a_rotate_written_at_the_wrong_generation_is_one_pdfium_follows_and_is_refused() {
    // THE REVIEWER'S REPRODUCTION. qpdf resolves `6 1 R` to null and drops /Rotate, raising no
    // warning; PDFium finds object 6 by number. Measured here rather than argued: PDFium shows
    // the 300 x 400 page turned.
    let wrong = rotated_by(1, 0);
    assert_eq!(
        pdfium_size(&wrong),
        (400.0, 300.0),
        "PDFium no longer follows a wrong-generation reference, so the case this refusal is \
         tied to has changed and needs re-measuring"
    );
    refused_on_both(&wrong, "reference-to-nothing", "/Rotate 6 1 R over 6 0");

    // THE TWIN: the same reference at the right generation. Both readers turn the page, and the
    // redaction goes ahead.
    let right = rotated_by(0, 0);
    assert_eq!(pdfium_size(&right), (400.0, 300.0));
    redacts_on_both(&right, "/Rotate 6 0 R over 6 0");
}

#[test]
fn a_reference_at_generation_zero_to_an_object_headed_at_one_is_refused() {
    // THE SECOND SILENT SHAPE: `6 0 R` where the file declares and heads object 6 at generation
    // 1. It falls out of the same rule: qpdf has no `(6, 0)`.
    let bytes = rotated_by(0, 1);
    assert_eq!(
        pdfium_size(&bytes),
        (400.0, 300.0),
        "PDFium follows `6 0 R` to a `6 1 obj`"
    );
    refused_on_both(&bytes, "reference-to-nothing", "/Rotate 6 0 R over 6 1");
    // And its twin: written at 1, referred to at 1.
    redacts_on_both(&rotated_by(1, 1), "/Rotate 6 1 R over 6 1");
}

#[test]
fn a_reference_not_written_in_plain_digits_is_refused_one_shape_at_a_time() {
    // qpdf reads each of these as `6 0 R` (measured with the CLI before #227). The rule refuses
    // the shape rather than deciding what the reader meant.
    for shape in ["+6 0 R", "6 +0 R", "06 00 R", "6 %c\n0 R"] {
        let mut objects = page_objects(&format!("/Rotate {shape}"));
        objects.push((6, 0, "90".to_owned()));
        refused_on_both(
            &classic(&objects),
            "reference-unreadable",
            &format!("/Rotate {shape}"),
        );
    }
}

#[test]
fn a_bad_reference_inside_an_object_stream_is_refused() {
    // THE ONLY BAD REFERENCE IS A MEMBER'S: the page lives in an object stream, and its /Rotate is
    // `6 1 R` over a `6 0`. Nothing in the file body names generation 1, so a check that read
    // the raw bytes alone would pass this.
    let direct: Vec<Written> = page_objects("")
        .into_iter()
        .filter(|(number, _, _)| *number != 3)
        .chain([(6, 0, "90".to_owned())])
        .collect();
    let wrong = with_object_stream(&direct, 7, 0, &[(3, page("/Rotate 6 1 R"))]);
    let objstm = wrong
        .windows(b"/ObjStm".len())
        .position(|w| w == b"/ObjStm")
        .expect("an object stream");
    let generation_one: Vec<usize> = wrong
        .windows(b"6 1 R".len())
        .enumerate()
        .filter(|(_, w)| *w == b"6 1 R")
        .map(|(at, _)| at)
        .collect();
    assert!(
        !generation_one.is_empty() && generation_one.iter().all(|at| *at > objstm),
        "the fixture's only `6 1 R` must be inside the object stream: {generation_one:?}"
    );
    assert_eq!(
        pdfium_size(&wrong),
        (400.0, 300.0),
        "PDFium follows the member's reference"
    );
    refused_on_both(
        &wrong,
        "reference-to-nothing",
        "a member's /Rotate 6 1 R over 6 0",
    );

    // THE TWIN: the same object stream, the reference at the right generation.
    let right = with_object_stream(&direct, 7, 0, &[(3, page("/Rotate 6 0 R"))]);
    redacts_on_both(&right, "a member's /Rotate 6 0 R over 6 0");
}

#[test]
fn a_reference_to_an_absent_object_is_refused_which_over_refuses_on_purpose() {
    // THE DELIBERATE OVER-REFUSAL (ADR 0029's #227 amendment). An object absent from both readers
    // leaks nothing -- each reads null -- and is refused anyway: qpdf cannot tell it from a pair
    // the cross-reference lacks while PDFium finds the number, which is the case that leaks.
    let objects = page_objects("/Thing 99 0 R");
    refused_on_both(
        &classic(&objects),
        "reference-to-nothing",
        "a reference to an absent object",
    );
}

#[test]
fn a_null_declared_at_the_pair_written_redacts() {
    // THE NARROWING (owner, #227). The first rule refused every null and cost 2 of 99 real
    // documents, both dvipdfm's: the catalog's `/Threads` names an object whose whole value is
    // `null`. Both readers read that null; refusing it bought nothing. qpdf declares it at
    // exactly the pair written, which an absent pair never is -- measured below, on each engine.
    let mut objects = page_objects("");
    objects[0] = (
        1,
        0,
        "<< /Type /Catalog /Pages 2 0 R /Threads 6 0 R >>".to_owned(),
    );
    objects.push((6, 0, "null".to_owned()));
    let bytes = classic(&objects);
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    let redactor = crate::web::redact_testing::redactor(&web);
    let native = super::Qpdf
        .open_for_redaction(&bytes, &options())
        .unwrap()
        .0;
    let on_the_web = redactor.open_for_redaction(&bytes, &options()).unwrap().0;
    for (engine, declared, absent) in [
        (
            "natively",
            native.object(6, 0).unwrap().object().unwrap(),
            native.object(6, 1).unwrap().object().unwrap(),
        ),
        (
            "on the web",
            on_the_web.object(6, 0).unwrap().object().unwrap(),
            on_the_web.object(6, 1).unwrap().object().unwrap(),
        ),
    ] {
        assert_eq!(
            declared,
            (6, 0),
            "a declared null carries its identity, {engine}"
        );
        assert_eq!(absent, (0, 0), "an absent pair carries none, {engine}");
    }
    redacts_on_both(&bytes, "/Threads naming a declared null");
}

#[test]
fn an_object_stream_qpdf_cannot_load_is_refused_by_the_warning_check_not_this_rule() {
    // THE HOLE THE NARROWING MOVED, pinned where it is now caught. The object stream is headed and
    // cross-referenced at generation 1; qpdf reads members through `(7, 0)`, cannot load the
    // stream, warns, and stores member 6 as a null WITH its identity -- so the reference check
    // passes `/Rotate 6 0 R`, and only the warnings after the write refuse it. PDFium follows the
    // member and turns the page: the two readers do not agree, which is why this must refuse.
    let bytes = with_object_stream(
        &page_objects("/Rotate 6 0 R"),
        7,
        1,
        &[(6, "90".to_owned())],
    );
    assert_eq!(
        pdfium_size(&bytes),
        (400.0, 300.0),
        "PDFium no longer follows a member of a generation-1 object stream, so this case needs \
         re-measuring"
    );
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    let redactor = crate::web::redact_testing::redactor(&web);
    let native = super::Qpdf
        .open_for_redaction(&bytes, &options())
        .unwrap()
        .0;
    let on_the_web = redactor.open_for_redaction(&bytes, &options()).unwrap().0;
    // The type first, as the policy reads it: resolving the member is what makes qpdf warn.
    let natively = native.object(6, 0).unwrap();
    let web_member = on_the_web.object(6, 0).unwrap();
    for (engine, type_code, identity) in [
        ("natively", natively.type_code(), natively.object().unwrap()),
        (
            "on the web",
            web_member.type_code(),
            web_member.object().unwrap(),
        ),
    ] {
        assert_eq!(type_code, object_type::NULL, "{engine}");
        assert_eq!(
            identity,
            (6, 0),
            "the member qpdf could not load carries its identity, so this rule passes it, {engine}"
        );
    }
    refused_on_both(
        &bytes,
        "engine-repaired-input",
        "a member of an object stream qpdf cannot load",
    );
}
