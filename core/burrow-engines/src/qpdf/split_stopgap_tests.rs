//! Split's optional-content stopgap at its open, on both engines (owner, 2026-10-13).
//!
//! Split refuses any document whose catalog carries `/OCProperties`, after refusing any reference
//! qpdf resolves to nothing (#227's check, with split's own refusals). Each case is opened through
//! the native engine and through the web engine over the native-backed bridge, so the two paths
//! are held to the same answer. The documents are benign -- nothing in them is hidden -- because
//! what is pinned is the refusal, not a layer: a document with a layer configuration of any shape
//! refuses, and one without opens.

use std::sync::Arc;

use burrow_types::{Clock, Limits, ManualClock};

use super::references_tests::{
    Written, classic, glued_trailer, long_name, planted, with_object_stream, with_object_zero,
};
use super::web_differential_tests::NativeBridge;
use crate::{OpenOptions, PageExtractor};

fn options() -> OpenOptions<'static> {
    OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    )
}

/// What each engine's split open answers for `pdf`: `Ok` or the refusal's text.
fn opened(pdf: &[u8]) -> [Result<(), String>; 2] {
    let native = PageExtractor::open(
        &super::Qpdf::new(),
        pdf.to_vec().into_boxed_slice(),
        &options(),
    )
    .map(drop)
    .map_err(|error| error.to_string());
    let web = PageExtractor::open(
        &crate::web::WebQpdf::new(Arc::new(NativeBridge::new())),
        pdf.to_vec().into_boxed_slice(),
        &options(),
    )
    .map(drop)
    .map_err(|error| error.to_string());
    [native, web]
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8], rule: &str) {
    for (engine, result) in ["native", "web"].iter().zip(opened(pdf)) {
        match result {
            Err(error) => assert!(error.contains(rule), "{what}, {engine}: {error}"),
            Ok(()) => panic!("{what}, {engine}: opened, where {rule} must refuse"),
        }
    }
}

#[track_caller]
fn opens(what: &str, pdf: &[u8]) {
    for (engine, result) in ["native", "web"].iter().zip(opened(pdf)) {
        if let Err(error) = result {
            panic!("{what}, {engine}: {error}");
        }
    }
}

/// A one-page document with `catalog` as object 1, `page_extra` in the page dictionary, `content`
/// as its stream, and `more` after the five objects.
fn document(catalog: &str, page_extra: &str, content: &str, more: &[Written]) -> Vec<Written> {
    let mut objects = vec![
        (1, 0, catalog.to_owned()),
        (2, 0, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned()),
        (
            3,
            0,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R \
                 /Resources << /Font << /F1 5 0 R >> {page_extra} >> >>"
            ),
        ),
        (
            4,
            0,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ),
        (
            5,
            0,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        ),
    ];
    objects.extend_from_slice(more);
    objects
}

const SHOWN: &str = "BT /F1 24 Tf 20 350 Td (KEEP) Tj ET";
const PLAIN_CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R >>";
const GROUP: (u32, u16, &str) = (6, 0, "<< /Type /OCG /Name (Layer) >>");

fn group() -> Written {
    (GROUP.0, GROUP.1, GROUP.2.to_owned())
}

/// A LAYER CONFIGURATION OF ANY SHAPE refuses: a dictionary, the same behind a reference, an
/// array (no reader hides through one, and it refuses all the same), and the key written with a
/// `#` escape, which qpdf reads as the key itself.
#[test]
fn a_catalog_with_optional_content_properties_of_any_shape_refuses() {
    for (what, catalog, more) in [
        (
            "a dictionary",
            "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R] /D << /ON [6 0 R] >> >> >>",
            vec![group()],
        ),
        (
            "an indirect dictionary",
            "<< /Type /Catalog /Pages 2 0 R /OCProperties 7 0 R >>",
            vec![group(), (7, 0, "<< /OCGs [6 0 R] /D << >> >>".to_owned())],
        ),
        (
            "an array",
            "<< /Type /Catalog /Pages 2 0 R /OCProperties [6 0 R] >>",
            vec![group()],
        ),
        (
            "an escaped key",
            "<< /Type /Catalog /Pages 2 0 R /OCPr#6fperties << /OCGs [6 0 R] /D << >> >> >>",
            vec![group()],
        ),
    ] {
        refuses(
            what,
            &classic(&document(catalog, "", SHOWN, &more)),
            "[split-optional-content-document]",
        );
    }
}

/// WHEREVER THE CATALOG IS WRITTEN: inside an object stream, which a byte scan of the file body
/// would not see, and as a direct dictionary in the trailer, which has no object number.
#[test]
fn a_catalog_in_an_object_stream_or_in_the_trailer_is_read_as_well() {
    let catalog = "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [6 0 R] /D << >> >> >>";
    let direct: Vec<Written> = document(catalog, "", SHOWN, &[group()])
        .into_iter()
        .filter(|(number, _, _)| *number != 1)
        .collect();
    refuses(
        "in an object stream",
        &with_object_stream(&direct, 9, 0, &[(1, catalog.to_owned())]),
        "[split-optional-content-document]",
    );

    let mut pdf = classic(&document(PLAIN_CATALOG, "", SHOWN, &[group()]));
    let trailer = b"/Root 1 0 R";
    let at = pdf
        .windows(trailer.len())
        .rposition(|w| w == trailer)
        .expect("the trailer's /Root");
    let mut rewritten = pdf[..at].to_vec();
    rewritten.extend_from_slice(catalog.replacen("<<", "/Root <<", 1).as_bytes());
    rewritten.extend_from_slice(&pdf[at + trailer.len()..]);
    pdf = rewritten;
    refuses(
        "a direct catalog in the trailer",
        &pdf,
        "[split-optional-content-document]",
    );
}

/// A REFERENCE QPDF RESOLVES TO NOTHING refuses before the catalog is read: `/OCProperties 7 1 R`
/// over a file holding only `7 0`, which qpdf reads as absent -- so a check of the key alone
/// would pass it -- while a reader that ignores the generation finds the object (#227).
#[test]
fn a_catalog_key_qpdf_resolves_to_nothing_refuses_as_a_reference() {
    let pdf = classic(&document(
        "<< /Type /Catalog /Pages 2 0 R /OCProperties 7 1 R >>",
        "",
        SHOWN,
        &[group(), (7, 0, "<< /OCGs [6 0 R] /D << >> >>".to_owned())],
    ));
    refuses(
        "behind a wrong generation",
        &pdf,
        "[split-reference-to-nothing]",
    );
}

/// A CROSS-REFERENCE OFFSET THAT IS WRONG for the object `/OCProperties` names: qpdf reads it as
/// absent, and warns only when the key is read, while other readers still find object 7 (the
/// stopgap's round 0). #227's check asks qpdf for `7 0` first and gets the null, so it refuses
/// before the catalog is read -- the warning check after the read is reached by no shape measured.
#[test]
fn a_catalog_key_at_a_wrong_offset_refuses_as_a_reference() {
    let mut pdf = classic(&document(
        "<< /Type /Catalog /Pages 2 0 R /OCProperties 7 0 R >>",
        "",
        SHOWN,
        &[group(), (7, 0, "<< /OCGs [6 0 R] /D << >> >>".to_owned())],
    ));
    // Object 7's entry is the xref's eighth line after the header; point it two bytes late.
    let text = String::from_utf8(pdf.clone()).expect("ASCII");
    // At a line start: `startxref` ends in the same five bytes.
    let xref = text.rfind("\nxref\n").expect("an xref") + 1;
    let entry = text[xref..]
        .lines()
        .nth(2 + 7)
        .expect("object 7's entry")
        .to_owned();
    let offset: usize = entry[..10].parse().expect("an offset");
    let moved = format!("{:010}{}", offset + 2, &entry[10..]);
    let at = xref + text[xref..].find(&entry).expect("the entry");
    pdf.splice(at..at + entry.len(), moved.bytes());
    refuses("a wrong offset", &pdf, "[split-reference-to-nothing]");
}

/// A REPAIR THE REFERENCE PASS CAUSES refuses as the repair it is, not as optional content: the
/// generated fixture (`tools/check-redaction-corpus.sh` builds it; CI does before the tests) whose
/// font qpdf repairs only when it is resolved, which the pass resolves.
#[test]
fn a_repair_the_reference_pass_causes_is_named_as_a_repair() {
    let pdf = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/redaction/generated/evade-font-repaired-during-the-walk.pdf"),
    )
    .expect("the generated fixture: run tools/check-redaction-corpus.sh, which builds it");
    refuses("a repaired font", &pdf, "[engine-repaired-input]");
}

/// THE NEAR-MISSES open: a catalog with no `/OCProperties`; one whose `/OCProperties` is the
/// literal `null`, and one whose reference names an object the file declares as `null`, which
/// every reader reads as absent and #227's rule accepts -- the boundary a refusal of any present
/// key would cross; and a tagged document -- structure tree, `/MarkInfo`, a marked-content property
/// list on the page -- which is the shape most like optional content that carries none.
#[test]
fn a_document_without_optional_content_opens() {
    opens(
        "a plain catalog",
        &classic(&document(PLAIN_CATALOG, "", SHOWN, &[])),
    );
    opens(
        "a literal null",
        &classic(&document(
            "<< /Type /Catalog /Pages 2 0 R /OCProperties null >>",
            "",
            SHOWN,
            &[],
        )),
    );
    opens(
        "a reference to an object declared null",
        &classic(&document(
            "<< /Type /Catalog /Pages 2 0 R /OCProperties 7 0 R >>",
            "",
            SHOWN,
            &[(7, 0, "null".to_owned())],
        )),
    );
    opens(
        "a tagged document",
        &classic(&document(
            "<< /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 8 0 R >>",
            "/Properties << /P0 << /MCID 0 >> >>",
            "/P /P0 BDC BT /F1 24 Tf 20 350 Td (KEEP) Tj ET EMC",
            &[(8, 0, "<< /Type /StructTreeRoot >>".to_owned())],
        )),
    );
}

/// A NAME OUTSIDE THE MEASURED ALLOWLIST refuses at split's open (owner, 2026-10-14), before the
/// catalog is read: in the catalog, in a page's dictionary, followed by a vertical tab, longer than
/// 255 bytes written, and inside an object stream's member, which a scan of the file body would
/// not see. Benign documents: what is pinned is the refusal.
#[test]
fn a_name_outside_the_measured_allowlist_refuses_at_split_s_open() {
    const RULE: &str = "[split-name-read-differently]";
    for (what, catalog, page_extra, byte) in [
        (
            "0x80 in a catalog key",
            "<< /Type /Catalog /Pages 2 0 R /Ma`rk 1 >>",
            "",
            0x80,
        ),
        ("0xFF ending a page key", PLAIN_CATALOG, "/Mark` 1", 0xff),
        (
            "a key followed by a vertical tab",
            PLAIN_CATALOG,
            "/Mark`1",
            0x0b,
        ),
    ] {
        refuses(
            what,
            &planted(classic(&document(catalog, page_extra, SHOWN, &[])), byte),
            RULE,
        );
    }
    refuses(
        "256 bytes written",
        &classic(&document(
            PLAIN_CATALOG,
            &format!("{} 1", long_name(256)),
            SHOWN,
            &[],
        )),
        RULE,
    );
    let catalog = "<< /Type /Catalog /Pages 2 0 R /Ma`rk 1 >>";
    let direct: Vec<Written> = document(catalog, "", SHOWN, &[])
        .into_iter()
        .filter(|(number, _, _)| *number != 1)
        .collect();
    refuses(
        "0x80 in a key inside an object stream",
        &planted(
            with_object_stream(&direct, 9, 0, &[(1, catalog.to_owned())]),
            0x80,
        ),
        RULE,
    );
}

/// THE NEAR-MISSES open: the refused bytes written as escapes, which both read alike, and 255
/// bytes written.
#[test]
fn a_name_inside_the_measured_allowlist_opens_for_split() {
    for (what, page_extra) in [
        ("0x80 escaped", "/Ma#80rk 1".to_owned()),
        ("0xFF escaped", "/Mark#ff 1".to_owned()),
        ("0x0B escaped", "/Mark#0B 1".to_owned()),
        ("255 bytes written", format!("{} 1", long_name(255))),
    ] {
        opens(
            what,
            &classic(&document(PLAIN_CATALOG, &page_extra, SHOWN, &[])),
        );
    }
}

/// THE EMPTY NAME AND A MIXED-CASE ESCAPE refuse at split's open (#329; `name_reads_alike`),
/// in the page's `/Resources`. The twins open: a one-byte name, and the same escape in one case.
#[test]
fn the_empty_name_and_a_mixed_case_escape_refuse_split_s_open() {
    for (what, extra, twin) in [
        ("the empty name", "/ 1", "/A 1"),
        ("a mixed-case escape", "/Ma#aFrk 1", "/Ma#afrk 1"),
    ] {
        refuses(
            what,
            &classic(&document(PLAIN_CATALOG, extra, SHOWN, &[])),
            "[split-name-read-differently]",
        );
        opens(
            &format!("{what}: the twin"),
            &classic(&document(PLAIN_CATALOG, twin, SHOWN, &[])),
        );
    }
}

/// THE TWINS of the object-stream, string and glued-trailer cases: the same member written with an
/// escape opens; a refused byte inside a string that holds no `obj` opens; and a glued trailer
/// with a refused byte in a key refuses, its escaped twin opening.
#[test]
fn the_object_stream_string_and_glued_trailer_twins() {
    let catalog = |key: &str| format!("<< /Type /Catalog /Pages 2 0 R {key} >>");
    let direct: Vec<Written> = document(PLAIN_CATALOG, "", SHOWN, &[])
        .into_iter()
        .filter(|(number, _, _)| *number != 1)
        .collect();
    opens(
        "0x80 escaped in a key inside an object stream",
        &with_object_stream(&direct, 9, 0, &[(1, catalog("/Ma#80rk 1"))]),
    );
    opens(
        "0x80 inside a string",
        &planted(
            classic(&document(PLAIN_CATALOG, "/Mark (a`b)", SHOWN, &[])),
            0x80,
        ),
    );
    let trailer = |key: &str| {
        let text =
            String::from_utf8(classic(&document(PLAIN_CATALOG, "", SHOWN, &[]))).expect("ASCII");
        glued_trailer(
            text.replacen("/Root 1 0 R", &format!("/Root 1 0 R {key}"), 1)
                .into_bytes(),
        )
    };
    refuses(
        "0x80 in a glued trailer",
        &planted(trailer("/Ma`rk 1"), 0x80),
        "[split-name-read-differently]",
    );
    opens("0x80 escaped in a glued trailer", &trailer("/Ma#80rk 1"));
}

/// THE SAME MALFORMED SHAPES at split's open, here in the page's `/Resources`: qpdf warns, and the
/// open refuses the repair; the well-formed twins open.
#[test]
fn a_malformed_dictionary_around_a_name_refuses_split_s_open_as_a_repair() {
    for (what, extra, twin) in [
        (
            "a hex string where a key belongs",
            "/Mark /Va<41>",
            "/Mark /Va/Kx<41>",
        ),
        ("a brace after a key", "/Mark{ 1", "/Mark 1"),
    ] {
        refuses(
            what,
            &classic(&document(PLAIN_CATALOG, extra, SHOWN, &[])),
            "[engine-repaired-input]",
        );
        opens(
            &format!("{what}: the twin"),
            &classic(&document(PLAIN_CATALOG, twin, SHOWN, &[])),
        );
    }
}

/// A RAW VERTICAL TAB IN OBJECT SYNTAX refuses at split's open (owner, 2026-10-14), between tokens
/// and in a hex string; inside a literal string it opens.
#[test]
fn a_vertical_tab_in_object_syntax_refuses_split_s_open() {
    for (what, extra) in [
        ("between tokens", "/Mark 1`/Other 2"),
        ("in a hex string", "/Mark <41`42>"),
    ] {
        refuses(
            what,
            &planted(classic(&document(PLAIN_CATALOG, extra, SHOWN, &[])), 0x0b),
            "[split-syntax-read-differently]",
        );
    }
    opens(
        "a vertical tab inside a literal string",
        &planted(
            classic(&document(PLAIN_CATALOG, "/Mark (a`b)", SHOWN, &[])),
            0x0b,
        ),
    );
}

/// A NULL DICTIONARY VALUE refuses at split's open, and a direct key of the one catalog is the
/// exemption (owner, 2026-10-14). Minimal documents with a null value somewhere; nothing else in
/// them is unusual.
#[test]
fn a_null_dictionary_value_refuses_split_s_open_except_in_the_catalog() {
    const RULE: &str = "[split-null-value]";
    refuses(
        "a page dictionary key",
        &classic(&document(PLAIN_CATALOG, "/Extra null", SHOWN, &[])),
        RULE,
    );
    refuses(
        "a reference to an object declared null",
        &classic(&document(
            PLAIN_CATALOG,
            "/Extra 9 0 R",
            SHOWN,
            &[(9, 0, "null".to_owned())],
        )),
        RULE,
    );
    opens(
        "a direct catalog key",
        &classic(&document(
            "<< /Type /Catalog /Pages 2 0 R /Extra null >>",
            "",
            SHOWN,
            &[],
        )),
    );
    refuses(
        "a null one level below the catalog",
        &classic(&document(
            "<< /Type /Catalog /Pages 2 0 R /Extra << /Inner null >> >>",
            "",
            SHOWN,
            &[],
        )),
        RULE,
    );
    // The catalog's number written twice: an earlier revision of object 1 holding the null.
    let mut twice = vec![(
        1,
        0,
        "<< /Type /Catalog /Pages 2 0 R /Extra null >>".to_owned(),
    )];
    twice.extend(document(PLAIN_CATALOG, "", SHOWN, &[]));
    refuses(
        "the catalog's number occurring twice",
        &classic(&twice),
        RULE,
    );
    refuses(
        "a second /Type /Catalog object holding a null",
        &classic(&document(
            PLAIN_CATALOG,
            "",
            SHOWN,
            &[(
                9,
                0,
                "<< /Type /Catalog /Pages 2 0 R /Extra null >>".to_owned(),
            )],
        )),
        RULE,
    );
}

/// THE CATALOG'S IDENTITY, wherever it is written: as one object stream member its direct keys are
/// exempt; listed twice in one stream, or written directly in the trailer, they are not.
#[test]
fn the_catalog_exemption_holds_only_for_one_catalog_written_once() {
    const RULE: &str = "[split-null-value]";
    let with_null = "<< /Type /Catalog /Pages 2 0 R /Extra null >>";
    let direct: Vec<Written> = document(PLAIN_CATALOG, "", SHOWN, &[])
        .into_iter()
        .filter(|(number, _, _)| *number != 1)
        .collect();
    opens(
        "the catalog as one object stream member",
        &with_object_stream(&direct, 9, 0, &[(1, with_null.to_owned())]),
    );
    refuses(
        "the catalog listed twice in one object stream",
        &with_object_stream(
            &direct,
            9,
            0,
            &[(1, with_null.to_owned()), (1, PLAIN_CATALOG.to_owned())],
        ),
        RULE,
    );
    let text = String::from_utf8(classic(&document(PLAIN_CATALOG, "", SHOWN, &[]))).expect("ASCII");
    let in_trailer = text.replacen("/Root 1 0 R", &format!("/Root {with_null}"), 1);
    refuses(
        "a direct catalog in the trailer",
        in_trailer.as_bytes(),
        RULE,
    );
}

/// #252: A FORM WHOSE `/Length` IS WRONG, drawing text in a font from its own resources. Split's
/// open runs qpdf with recovery off, so qpdf reads the form as null rather than repairing it, and the
/// reference pass refuses the reference that names it; a part can no longer arrive without the
/// form's font. The twin, the same form with a correct `/Length`, opens. The length is off by two,
/// not one: one byte over lands on the end-of-line before `endstream` and opens on both engines
/// (measured, 2026-10-14), while one byte short refuses as two does.
#[test]
fn a_form_with_a_wrong_length_refuses_at_split_s_open() {
    let build = |delta: i64| {
        let body = "BT /F2 12 Tf 10 10 Td (FORM) Tj ET";
        let length = i64::try_from(body.len()).expect("small") + delta;
        let objects = document(
            PLAIN_CATALOG,
            "/XObject << /Fm 6 0 R >>",
            "q /Fm Do Q BT /F1 24 Tf 20 350 Td (KEEP) Tj ET",
            &[
                (
                    6,
                    0,
                    format!(
                        "<< /Type /XObject /Subtype /Form /BBox [0 0 300 400] /Resources << /Font << /F2 7 0 R >> >> /Length {length} >>\nstream\n{body}\nendstream"
                    ),
                ),
                (
                    7,
                    0,
                    "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>".to_owned(),
                ),
            ],
        );
        classic(&objects)
    };
    for delta in [-2, 2] {
        refuses(
            &format!("a form /Length off by {delta}"),
            &build(delta),
            "[split-reference-to-nothing]",
        );
    }
    opens("a form with a correct /Length", &build(0));
}

/// THE CATALOG'S NUMBER MUST BE POSITIVE to be exempt (#333), at split's open: a direct catalog in
/// the trailer is (0, 0), and an object headed `0 0 obj` that qpdf never reads must not stand in
/// for it. The twin holds no null.
#[test]
fn an_object_numbered_zero_is_never_the_catalog_at_split_s_open() {
    let text = String::from_utf8(classic(&document(PLAIN_CATALOG, "", SHOWN, &[]))).expect("ASCII");
    let text = text.replacen("/Root 1 0 R", &format!("/Root {PLAIN_CATALOG}"), 1);
    refuses(
        "a null key in an object headed 0 0 obj",
        &with_object_zero(&text, "<< /Extra null >>"),
        "[split-null-value]",
    );
    opens(
        "the twin, with no null",
        &with_object_zero(&text, "<< /Extra 1 >>"),
    );
}
