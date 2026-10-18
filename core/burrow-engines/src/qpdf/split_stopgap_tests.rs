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

/// THE CHROME SHAPE (owner, 2026-10-15): an empty key at the top level of the catalog's `/Dests`
/// dictionary, which Chrome's print-to-PDF writes for a link to an unnamed anchor, opens for split
/// when the catalog's `/Dests` entry is that dictionary's **sole referrer**, so nothing a part
/// receives can reach it; and nowhere else.
#[test]
fn an_empty_key_opens_split_only_in_a_dests_the_catalog_alone_refers_to() {
    const DESTS_CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R /Dests 6 0 R >>";
    const DESTS: &str = "<< / [3 0 R /XYZ 0 400 0] /Named [3 0 R /Fit] >>";
    let dests = |body: &str| vec![(6, 0, body.to_owned())];
    opens(
        "an empty key of a /Dests only the catalog names",
        &classic(&document(DESTS_CATALOG, "", SHOWN, &dests(DESTS))),
    );
    opens(
        "an empty key of a /Dests written in the catalog",
        &classic(&document(
            "<< /Type /Catalog /Pages 2 0 R /Dests << / [3 0 R /XYZ 0 400 0] >> >>",
            "",
            SHOWN,
            &[],
        )),
    );
    let form = "0 0 m 1 1 l S";
    let form_sharing = vec![
        (6, 0, DESTS.to_owned()),
        (
            7,
            0,
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /Extra 6 0 R >> \
                 /Length {} >>\nstream\n{form}\nendstream",
                form.len()
            ),
        ),
    ];
    // THE TWINS: the same /Dests also reached another way, one level down, not the catalog's, in
    // another dictionary, or the catalog itself.
    for (what, catalog, page_extra, more) in [
        (
            "a form's /Resources also names /Dests",
            DESTS_CATALOG,
            "/XObject << /X1 7 0 R >>",
            form_sharing,
        ),
        (
            "the page's /XObject dictionary is /Dests",
            DESTS_CATALOG,
            "/XObject 6 0 R",
            dests(DESTS),
        ),
        (
            "the page's /ExtGState dictionary is /Dests",
            DESTS_CATALOG,
            "/ExtGState 6 0 R",
            dests(DESTS),
        ),
        (
            "a name tree also names /Dests",
            "<< /Type /Catalog /Pages 2 0 R /Dests 6 0 R /Names << /Dests 7 0 R >> >>",
            "",
            vec![
                (6, 0, DESTS.to_owned()),
                (7, 0, "<< /Names [(a) 6 0 R] >>".to_owned()),
            ],
        ),
        (
            "one level down in /Dests",
            DESTS_CATALOG,
            "",
            dests("<< /A << / [3 0 R /XYZ 0 400 0] >> >>"),
        ),
        (
            "/Dests not named by the catalog",
            PLAIN_CATALOG,
            "/Extra 6 0 R",
            dests("<< / [3 0 R /XYZ 0 400 0] >>"),
        ),
        (
            "another dictionary, beside a /Dests",
            DESTS_CATALOG,
            "/Extra 7 0 R",
            vec![
                (6, 0, "<< /Named [3 0 R /Fit] >>".to_owned()),
                (7, 0, "<< / [3 0 R /XYZ 0 400 0] >>".to_owned()),
            ],
        ),
        (
            "the catalog is its own /Dests",
            "<< /Type /Catalog /Pages 2 0 R /Dests 1 0 R / [3 0 R /XYZ 0 400 0] >>",
            "",
            Vec::new(),
        ),
    ] {
        refuses(
            what,
            &classic(&document(catalog, page_extra, SHOWN, &more)),
            "[split-name-read-differently]",
        );
    }
}

/// THE CATALOG NAMED BY AN OBJECT a part may copy carries its `/Dests` with it, so an empty key is
/// admitted only when nothing but a trailer names the catalog (round 2's review). Each shape
/// refuses at the OPEN -- these tests never extract, so the part re-read cannot be what refused
/// them -- and for a direct `/Dests` as for an indirect one.
#[test]
fn an_empty_dests_key_refuses_at_the_open_when_an_object_names_the_catalog() {
    const DESTS: &str = "<< / [3 0 R /XYZ 0 400 0] /Named [3 0 R /Fit] >>";
    let form = "0 0 m 1 1 l S";
    for (what, catalog) in [
        (
            "an indirect /Dests",
            "<< /Type /Catalog /Pages 2 0 R /Dests 6 0 R >>",
        ),
        (
            "a direct /Dests",
            "<< /Type /Catalog /Pages 2 0 R /Dests << / [3 0 R /XYZ 0 400 0] >> >>",
        ),
    ] {
        for (shape, page_extra, content, more) in [
            (
                "a used form's /Resources names the catalog",
                "/XObject << /X1 7 0 R >>",
                "BT /F1 24 Tf 20 350 Td (KEEP) Tj ET /X1 Do",
                vec![(
                    7,
                    0,
                    format!(
                        "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] \
                         /Resources << /Extra 1 0 R >> /Length {} >>\nstream\n{form}\nendstream",
                        form.len()
                    ),
                )],
            ),
            (
                "a used /ExtGState names the catalog",
                "/ExtGState << /G 1 0 R >>",
                "/G gs BT /F1 24 Tf 20 350 Td (KEEP) Tj ET",
                Vec::new(),
            ),
            (
                "an annotation names the catalog",
                "/Extra 7 0 R",
                SHOWN,
                vec![(
                    7,
                    0,
                    "<< /Type /Annot /Subtype /Text /Rect [200 20 220 40] /Foo 1 0 R >>".to_owned(),
                )],
            ),
        ] {
            let mut objects = more.clone();
            objects.push((6, 0, DESTS.to_owned()));
            refuses(
                &format!("{what}: {shape}"),
                &classic(&document(catalog, page_extra, content, &objects)),
                "[split-name-read-differently]",
            );
        }
    }
    // A `/Dests` WRITTEN DIRECTLY IN ANOTHER OBJECT'S DICTIONARY is not the catalog's -- beside a
    // catalog whose own `/Dests` would be admitted, so only the holder tells them apart.
    refuses(
        "a direct /Dests in another object",
        &classic(&document(
            "<< /Type /Catalog /Pages 2 0 R /Dests 6 0 R >>",
            "/Extra 7 0 R",
            SHOWN,
            &[
                (6, 0, DESTS.to_owned()),
                (7, 0, "<< /Dests << / [3 0 R /Fit] >> >>".to_owned()),
            ],
        )),
        "[split-name-read-differently]",
    );
}

/// `/DESTS WRITTEN ONCE`, as the catalog is: a second revision of it in an incremental update, or a
/// header and an object-stream member both claiming its number, refuses; so does a `/Dests` naming
/// a generation the file does not head, as a reference to nothing.
#[test]
fn an_empty_dests_key_is_admitted_only_when_dests_is_written_once() {
    const DESTS_CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R /Dests 6 0 R >>";
    const DESTS: &str = "<< / [3 0 R /XYZ 0 400 0] >>";
    let base = classic(&document(
        DESTS_CATALOG,
        "",
        SHOWN,
        &[(6, 0, DESTS.to_owned())],
    ));
    refuses(
        "/Dests rewritten in an incremental update",
        &with_revision(&base, 6, DESTS),
        "[split-name-read-differently]",
    );
    let direct = document(DESTS_CATALOG, "", SHOWN, &[(6, 0, DESTS.to_owned())]);
    refuses(
        "/Dests headed and also an object-stream member",
        &with_object_stream(&direct, 9, 0, &[(6, DESTS.to_owned())]),
        "[split-name-read-differently]",
    );
    let wrong = classic(&document(
        "<< /Type /Catalog /Pages 2 0 R /Dests 6 1 R >>",
        "",
        SHOWN,
        &[(6, 0, DESTS.to_owned())],
    ));
    // qpdf resolves a generation the file does not head to nothing, so this refuses as such before
    // the empty key is judged; `written_once`'s generation branch is witnessed by name in
    // `redact::references::tests`.
    refuses(
        "a /Dests at a generation the file does not head",
        &wrong,
        "[split-reference-to-nothing]",
    );
}

/// `pdf`, a [`classic`] file, with `key` added to its trailer.
fn with_trailer_key(pdf: &[u8], key: &str) -> Vec<u8> {
    let text = String::from_utf8(pdf.to_vec()).expect("ASCII");
    assert!(text.contains("/Root 1 0 R >>"), "the trailer to add to");
    let out = text.replacen("/Root 1 0 R >>", &format!("/Root 1 0 R {key} >>"), 1);
    assert_ne!(out, text, "the trailer key was added");
    out.into_bytes()
}

/// A TRAILER THAT ALSO NAMES `/Dests` refuses the empty key: the trailer is a referrer, so the
/// catalog's entry is no longer the dictionary's sole one. The twin names an object that is not the
/// `/Dests` dictionary, and the same document opens.
#[test]
fn an_empty_dests_key_refuses_when_the_trailer_also_names_dests() {
    const DESTS_CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R /Dests 6 0 R >>";
    let base = classic(&document(
        DESTS_CATALOG,
        "",
        SHOWN,
        &[(
            6,
            0,
            "<< / [3 0 R /XYZ 0 400 0] /Named [3 0 R /Fit] >>".to_owned(),
        )],
    ));
    opens("the base document", &base);
    opens(
        "the twin: the trailer names another object",
        &with_trailer_key(&base, "/X 5 0 R"),
    );
    refuses(
        "the trailer also names /Dests",
        &with_trailer_key(&base, "/X 6 0 R"),
        "[split-name-read-differently]",
    );
}

/// A CROSS-REFERENCE STREAM'S `/Root` counts as an object naming the catalog, so the empty
/// `/Dests` key refuses in a file whose only `/Root` lives there: a RECORDED OVER-REFUSAL (it fails
/// closed), pinned so that a later relaxation shows up here as an acceptance. The twins: the same
/// file with a named key opens, and the same empty key behind a classic trailer opens.
#[test]
fn an_empty_dests_key_refuses_behind_an_xref_stream_root_as_a_recorded_over_refusal() {
    const DESTS_CATALOG: &str = "<< /Type /Catalog /Pages 2 0 R /Dests 6 0 R >>";
    let file = |dests: &str| {
        with_object_stream(
            &document(DESTS_CATALOG, "", SHOWN, &[(6, 0, dests.to_owned())]),
            9,
            0,
            &[(7, "<< /Harmless true >>".to_owned())],
        )
    };
    opens(
        "the twin: a named key behind an xref stream",
        &file("<< /A [3 0 R /XYZ 0 400 0] >>"),
    );
    opens(
        "the twin: the empty key behind a classic trailer",
        &classic(&document(
            DESTS_CATALOG,
            "",
            SHOWN,
            &[(6, 0, "<< / [3 0 R /XYZ 0 400 0] >>".to_owned())],
        )),
    );
    refuses(
        "the empty key, /Root in an xref stream",
        &file("<< / [3 0 R /XYZ 0 400 0] >>"),
        "[split-name-read-differently]",
    );
}

/// `base` with an incremental update rewriting object `number` as `body`.
fn with_revision(base: &[u8], number: i32, body: &str) -> Vec<u8> {
    let text = String::from_utf8_lossy(base);
    let prev: usize = text
        .rsplit("startxref")
        .next()
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|digits| digits.parse().ok())
        .expect("a startxref");
    let size = text
        .split("/Size ")
        .nth(1)
        .and_then(|tail| tail.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|digits| digits.parse::<usize>().ok())
        .expect("a /Size");
    let mut out = base.to_vec();
    let at = out.len();
    out.extend_from_slice(format!("{number} 0 obj\n{body}\nendobj\n").as_bytes());
    let xref = out.len();
    out.extend_from_slice(
        format!(
            "xref\n{number} 1\n{at:010} 00000 n \ntrailer\n<< /Size {size} /Root 1 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .as_bytes(),
    );
    out
}

/// THE SECOND LINE OF DEFENCE (owner, 2026-10-15): with the sole-referrer condition taken out
/// under `cfg(test)`, a /Dests a form's resources also name is admitted at the open, and the part
/// that carries it is refused when it is written, on both engines. No measured shape reaches it
/// otherwise.
#[test]
fn a_part_carrying_an_admitted_empty_key_is_refused_when_written() {
    let catalog = "<< /Type /Catalog /Pages 2 0 R /Dests 6 0 R >>";
    let form = "0 0 m 1 1 l S";
    let more = vec![
        (
            6,
            0,
            "<< / [3 0 R /XYZ 0 400 0] /Named [3 0 R /Fit] >>".to_owned(),
        ),
        (
            7,
            0,
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /Extra 6 0 R >> \
                 /Length {} >>\nstream\n{form}\nendstream",
                form.len()
            ),
        ),
    ];
    let pdf = classic(&document(
        catalog,
        "/XObject << /X1 7 0 R >>",
        "BT /F1 24 Tf 20 350 Td (KEEP) Tj ET /X1 Do",
        &more,
    ));
    crate::redact::hooks::SOLE_REFERRER_SKIPPED.with(|flag| flag.set(true));
    let native = super::Qpdf::new();
    let web = crate::web::WebQpdf::new(Arc::new(NativeBridge::new()));
    // THE OPEN ADMITS IT, on both, so the refusal below is the part's and not the open's.
    let native_source = PageExtractor::open(&native, pdf.clone().into_boxed_slice(), &options());
    let web_source = PageExtractor::open(&web, pdf.into_boxed_slice(), &options());
    let native_part = native_source
        .as_ref()
        .map_err(ToString::to_string)
        .map(|source| PageExtractor::extract(&native, source, 0, 1, &options()));
    let web_part = web_source
        .as_ref()
        .map_err(ToString::to_string)
        .map(|source| PageExtractor::extract(&web, source, 0, 1, &options()));
    crate::redact::hooks::SOLE_REFERRER_SKIPPED.with(|flag| flag.set(false));
    let native_part = native_part.unwrap_or_else(|error| panic!("native open refused: {error}"));
    let web_part = web_part.unwrap_or_else(|error| panic!("web open refused: {error}"));
    for (engine, part) in [("native", native_part), ("web", web_part)] {
        match part {
            Err(error) => assert!(
                error.to_string().contains("[split-name-read-differently]"),
                "{engine}: refused, but not by the empty-key re-read: {error}"
            ),
            Ok(_) => panic!("{engine}: a part carrying the empty key was written"),
        }
    }
}

/// THE EMPTY NAME AS A VALUE, as Chrome writes it in a link annotation: `/Dest /` and a line break,
/// admitted; under a key whose value means something, refused (owner, 2026-10-15).
#[test]
fn an_empty_dest_in_a_link_opens_split_and_a_meaningful_key_refuses() {
    let link = |inside: &str| {
        vec![(
            6,
            0,
            format!("<< /Type /Annot /Subtype /Link /Rect [200 20 260 40] /F 4 {inside} >>"),
        )]
    };
    opens(
        "/Dest / in a link",
        &classic(&document(
            PLAIN_CATALOG,
            "",
            SHOWN,
            &link("/Dest /\n/Border [0 0 0]"),
        )),
    );
    for (what, inside) in [("/S /", "/S / /Dest /A"), ("/Subtype /", "/Subtype / ")] {
        refuses(
            what,
            &classic(&document(PLAIN_CATALOG, "", SHOWN, &link(inside))),
            "[split-name-read-differently]",
        );
    }
}

/// THE UNTAGGED CHROME SHAPE (#358, owner 2026-10-16): Chrome writes `/Dest /` as an annotation's
/// last entry, so the empty value is followed directly by the `>>` that closes it. Admitted. The
/// near-miss twin, an empty KEY before the same `>>`, refuses, as does any follower Chrome does
/// not write, and a `>` that does not begin `>>`.
#[test]
fn an_empty_dest_closing_its_link_opens_split_and_an_empty_key_there_refuses() {
    let link = |inside: &str| {
        vec![(
            6,
            0,
            format!("<< /Type /Annot /Subtype /Link /Rect [200 20 260 40] /F 4 {inside}"),
        )]
    };
    for inside in ["/Border [0 0 0]\n/Dest />>", "/Dest />>"] {
        opens(
            &format!("untagged Chrome: {inside:?}"),
            &classic(&document(PLAIN_CATALOG, "", SHOWN, &link(inside))),
        );
    }
    for inside in [
        "/Dest [3 0 R /Fit] />>",
        "/Dest /> >>",
        "/Dest / >>",
        "/Dest /\t>>",
        "/Dest /\r>>",
    ] {
        refuses(
            &format!("twin: {inside:?}"),
            &classic(&document(PLAIN_CATALOG, "", SHOWN, &link(inside))),
            "[split-name-read-differently]",
        );
    }
}

/// WHERE THE EMPTY `/Dest` VALUE MAY STAND (owner, 2026-10-17): at the top level of a link
/// annotation's own dictionary, where Chrome writes it, and nowhere else. One level down -- in a
/// dictionary a link holds, or in one a used form's dictionary holds -- at the top level of a used
/// form's dictionary, which is a stream's, or in an annotation that is not a link, one with no
/// `/Type`, or a stream whose dictionary reads as a link's, it refuses, before each follower (LF,
/// and the `>>` closing its dictionary). After an integer value it is the next key, not the value,
/// and refuses before LF too. The twins, the same value at a link's top level, open.
#[test]
fn an_empty_dest_value_opens_split_only_at_a_link_annotations_top_level() {
    const FORM_SHOWN: &str = "BT /F1 24 Tf 20 350 Td (KEEP) Tj ET /X1 Do";
    let annotation = |opening: &str, inside: &str| {
        vec![(
            6,
            0,
            format!("<< {opening} /Rect [200 20 260 40] /F 4 {inside}"),
        )]
    };
    let link = |inside: &str| annotation("/Type /Annot /Subtype /Link", inside);
    let form = |inside: &str| {
        let drawn = "0 0 m 1 1 l S";
        vec![(
            7,
            0,
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length {} {inside}\n\
                 stream\n{drawn}\nendstream",
                drawn.len()
            ),
        )]
    };
    let in_link = |more: &[Written]| classic(&document(PLAIN_CATALOG, "", SHOWN, more));
    let in_form = |more: &[Written]| {
        classic(&document(
            PLAIN_CATALOG,
            "/XObject << /X1 7 0 R >>",
            FORM_SHOWN,
            more,
        ))
    };
    for (what, pdf) in [
        (
            "nested in a link, before LF",
            in_link(&link("/A << /S /GoTo /Dest /\n>> >>")),
        ),
        (
            "nested in a link, before >>",
            in_link(&link("/A << /S /GoTo /Dest />> >>")),
        ),
        (
            "nested in a used form, before LF",
            in_form(&form("/Info << /Dest /\n>> >>")),
        ),
        (
            "nested in a used form, before >>",
            in_form(&form("/Info << /Dest />> >>")),
        ),
        (
            "a used form's top level, before LF",
            in_form(&form("/Dest /\n>>")),
        ),
        (
            "a used form's top level, before >>",
            in_form(&form("/Dest />>")),
        ),
        (
            "an annotation that is not a link, before LF",
            in_link(&annotation("/Type /Annot /Subtype /Text", "/Dest /\n>>")),
        ),
        (
            "an annotation that is not a link, before >>",
            in_link(&annotation("/Type /Annot /Subtype /Text", "/Dest />>")),
        ),
        (
            "a link with no /Type, before LF",
            in_link(&annotation("/Subtype /Link", "/Dest /\n>>")),
        ),
        (
            "a link with no /Type, before >>",
            in_link(&annotation("/Subtype /Link", "/Dest />>")),
        ),
        (
            "a stream whose dictionary reads as a link's, before LF",
            in_link(&[(
                6,
                0,
                "<< /Type /Annot /Subtype /Link /Rect [200 20 260 40] /Dest /\n/Length 0 >>\n\
                 stream\n\nendstream"
                    .to_owned(),
            )]),
        ),
        (
            "a stream whose dictionary reads as a link's, before >>",
            in_link(&[(
                6,
                0,
                "<< /Type /Annot /Subtype /Link /Rect [200 20 260 40] /Length 0 /Dest />>\n\
                 stream\n\nendstream"
                    .to_owned(),
            )]),
        ),
        (
            "after an integer value, before LF: the next key",
            in_link(&link("/Dest 5 /\n6 >>")),
        ),
    ] {
        refuses(what, &pdf, "[split-name-read-differently]");
    }
    // THE TWINS: the same value at a link's top level, on both followers, and the forms above
    // with a named /Dest, open.
    for (what, pdf) in [
        (
            "a link's top level, before LF",
            in_link(&link("/Dest /\n>>")),
        ),
        ("a link's top level, before >>", in_link(&link("/Dest />>"))),
        (
            "nested in a link, named",
            in_link(&link("/A << /S /GoTo /Dest /A\n>> >>")),
        ),
        (
            "a used form's top level, named",
            in_form(&form("/Dest /A\n>>")),
        ),
        (
            "after an integer value before another key, before LF",
            in_link(&link("/K 5 /Dest /\n>>")),
        ),
        (
            "a stream whose dictionary reads as a link's, named",
            in_link(&[(
                6,
                0,
                "<< /Type /Annot /Subtype /Link /Rect [200 20 260 40] /Dest /A\n/Length 0 >>\n\
                 stream\n\nendstream"
                    .to_owned(),
            )]),
        ),
    ] {
        opens(what, &pdf);
    }
}

/// A LINK WRITTEN TWICE is not one whose empty `/Dest` value split admits (round 1's reviews): an
/// incremental update that writes the same link again leaves two copies of the object, and which
/// one a reader takes is not something the rule reasons about, so it refuses. The twins open: the
/// same link written once, and the same link written twice with a named `/Dest`, so it is the empty
/// value that refuses, not the update.
#[test]
fn an_empty_dest_value_opens_split_only_in_a_link_written_once() {
    let link = |dest: &str| {
        format!("<< /Type /Annot /Subtype /Link /Rect [200 20 260 40] /F 4 /Dest {dest}\n>>")
    };
    let once = |dest: &str| classic(&document(PLAIN_CATALOG, "", SHOWN, &[(6, 0, link(dest))]));
    refuses(
        "an empty /Dest value in a link written twice",
        &with_revision(&once("/"), 6, &link("/")),
        "[split-name-read-differently]",
    );
    opens("the twin: the same link written once", &once("/"));
    opens(
        "the twin: a named /Dest in a link written twice",
        &with_revision(&once("/A"), 6, &link("/A")),
    );
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

/// A clock that counts how often it is read and moves a millisecond each time.
struct Counting(std::sync::atomic::AtomicU64);
impl Clock for Counting {
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

/// THE EMPTY-VALUE LOOKUPS READ THE DEADLINE (round 1's security review): split's reference pass
/// asks the engine about every link annotation holding an empty `/Dest` value, one question per
/// object, and reads the deadline between them as the reference lookups do. Asked directly, with a
/// clock that moves a millisecond per read: a document of many such links reads it more often than
/// its twin holding a number in the same place, and with the budget set between the two counts, the
/// pass refuses with the time limit -- which, without the reads in that loop, it never could, since
/// nothing after the loop reads the clock.
#[test]
fn the_empty_value_lookups_read_the_deadline() {
    use crate::redact::graph::OpensForRedaction;

    let links: u32 = 4096;
    let build = |dest: &str| {
        let mut more: Vec<Written> = Vec::new();
        for number in 0..links {
            more.push((
                100 + number,
                0,
                format!("<< /Type /Annot /Subtype /Link /Rect [200 20 260 40] /Dest {dest}\n>>"),
            ));
        }
        classic(&document(PLAIN_CATALOG, "", SHOWN, &more))
    };
    let run = |pdf: &[u8], budget: u64| {
        let (document, _) = super::Qpdf
            .open_for_redaction(pdf, &options())
            .expect("opens");
        let clock = Counting(std::sync::atomic::AtomicU64::new(0));
        let mut limits = Limits::default();
        limits.max_duration_ms = budget;
        let deadline = burrow_types::Deadline::start(&clock, &limits);
        let result = crate::redact::references::refuse_references_to_nothing_as(
            &document,
            pdf,
            &deadline,
            &clock,
            &crate::prune::SPLIT_REFERENCES,
        )
        .map(drop);
        (result, clock.0.load(std::sync::atomic::Ordering::Relaxed))
    };
    let empty = build("/");
    let twin = build("0");
    assert_eq!(empty.len(), twin.len(), "the twin is the same length");
    let (twin_result, twin_reads) = run(&twin, u64::MAX);
    twin_result.expect("the twin passes");
    let (empty_result, empty_reads) = run(&empty, u64::MAX);
    empty_result.expect("links at their top level pass with time to spare");
    let loop_reads = empty_reads.saturating_sub(twin_reads);
    assert!(
        loop_reads >= u64::from(links) / 256,
        "{loop_reads} more deadline reads over {links} links holding an empty /Dest value: the \
         lookups must read it every 256 objects"
    );
    let budget = twin_reads + loop_reads / 2;
    match run(&empty, budget).0 {
        Err(burrow_types::Error::LimitExceeded { limit, .. }) => {
            assert_eq!(limit, "max_duration_ms");
        }
        other => panic!("a deadline that passes inside the lookups: {other:?}"),
    }
}
