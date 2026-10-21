//! Split keeps every page-level graphics state whole, and refuses an unused one that could carry
//! content, on both engines (owner, 2026-10-19; ADR 0019 §2b's graphics-state amendment).
//!
//! Each case is split through the native engine and through the web engine over the native-backed
//! bridge, so the two paths are held to the same answer: both reach the one policy in
//! `crate::prune`. A refusal is asserted by its rule id; an `Ok` by the part existing and, where it
//! is the point, by what the part's graphics-state dictionary still holds.
//!
//! The documents here are two pages, and the part is page 1 unless a case says otherwise. Page 2
//! draws with `/GSX`; page 1 draws without it unless a case says so.

use std::sync::Arc;

use burrow_types::{Clock, Limits, ManualClock};

use super::references_tests::{Written, classic};
use super::web_differential_tests::NativeBridge;
use crate::{OpenOptions, PageExtractor};

const UNUSED: &str = "[split-graphics-state-unused]";
const NOT_A_DICTIONARY: &str = "[split-resource-category-not-a-dictionary]";

fn options_with(clock: Arc<dyn Clock>) -> OpenOptions<'static> {
    OpenOptions::new(Limits::default(), clock)
}

fn options() -> OpenOptions<'static> {
    options_with(Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

/// One engine's part: pages `first .. first + count` of `pdf`, or the refusal's text.
fn part_on<E: PageExtractor>(
    engine: &E,
    pdf: &[u8],
    first: u64,
    count: u64,
    options: &OpenOptions<'_>,
) -> Result<Vec<u8>, String> {
    let source = engine
        .open(pdf.to_vec().into_boxed_slice(), options)
        .map_err(|error| error.to_string())?;
    engine
        .extract(&source, first, count, options)
        .map_err(|error| error.to_string())
}

/// The part on each engine, native first.
fn parts(pdf: &[u8], first: u64, count: u64) -> [Result<Vec<u8>, String>; 2] {
    [
        part_on(&super::Qpdf::new(), pdf, first, count, &options()),
        part_on(
            &crate::web::WebQpdf::new(Arc::new(NativeBridge::new())),
            pdf,
            first,
            count,
            &options(),
        ),
    ]
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8], first: u64, count: u64, rule: &str) {
    for (engine, result) in ["native", "web"].iter().zip(parts(pdf, first, count)) {
        match result {
            Err(error) => assert!(error.contains(rule), "{what}, {engine}: {error}"),
            Ok(_) => panic!("{what}, {engine}: split, where {rule} must refuse"),
        }
    }
}

/// Split, on both engines; the native part is returned.
#[track_caller]
fn splits(what: &str, pdf: &[u8], first: u64, count: u64) -> Vec<u8> {
    let [native, web] = parts(pdf, first, count);
    if let Err(error) = &web {
        panic!("{what}, web: {error}");
    }
    native.unwrap_or_else(|error| panic!("{what}, native: {error}"))
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

fn stream(dictionary: &str, data: &str) -> String {
    format!(
        "<< {dictionary} /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

const HELVETICA: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
const PAGE_ONE: &str = "BT /F1 24 Tf 20 300 Td (PAGEONE) Tj ET";
const PAGE_TWO: &str = "q /GSX gs 0 g 0 0 300 400 re f Q BT /F1 24 Tf 20 200 Td (PAGETWO) Tj ET";
const PAGE_ONE_USES: &str =
    "q /GSX gs 0 g 0 0 300 400 re f Q BT /F1 24 Tf 20 300 Td (PAGEONE) Tj ET";
const OWN_RESOURCES: &str = "/Resources << /Font << /F1 7 0 R >> /ExtGState << /GSX 8 0 R >> >>";
const FONT_ONLY: &str = "/Resources << /Font << /F1 7 0 R >> >>";

/// A soft mask's group form: what a mask graphics state reaches.
fn mask_form() -> String {
    stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Group << /S /Transparency /CS \
         /DeviceGray >> /Resources << /Font << /F1 7 0 R >> >>",
        "1 g 0 0 300 400 re f 0 g BT /F1 24 Tf 20 200 Td (MASKFORM) Tj ET",
    )
}

/// A sampled function: what a transfer, black-generation or undercolour-removal entry reaches.
fn sampled() -> String {
    stream(
        "/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8",
        "AB",
    )
}

/// How the two pages come to list the same graphics state.
#[derive(Clone, Copy, Debug)]
enum Sharing {
    /// Inherited: one `/Resources` on the page tree's root.
    Inherited,
    /// Each page's own `/Resources`, one shared `/ExtGState` dictionary.
    Category,
    /// Each page's own `/Resources` and `/ExtGState`, one shared graphics-state object.
    Entry,
}

/// Two pages; object 8 is the graphics state `/GSX`, `extra` are objects 9 onwards.
struct Two<'a> {
    state: &'a str,
    extra: Vec<String>,
    page_one: &'a str,
    page_two: &'a str,
    resources_one: Option<String>,
    resources_two: Option<String>,
    pages: Option<String>,
}

impl<'a> Two<'a> {
    fn new(state: &'a str, extra: Vec<String>) -> Self {
        Self {
            state,
            extra,
            page_one: PAGE_ONE,
            page_two: PAGE_TWO,
            resources_one: None,
            resources_two: None,
            pages: None,
        }
    }

    fn sharing(mut self, sharing: Sharing) -> Self {
        match sharing {
            Sharing::Inherited => {
                self.pages = Some(format!(
                    "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] {OWN_RESOURCES} >>"
                ));
                self.resources_one = Some(String::new());
                self.resources_two = Some(String::new());
            }
            Sharing::Category => {
                let at = 9 + self.extra.len();
                self.extra.push("<< /GSX 8 0 R >>".to_owned());
                let resources =
                    format!("/Resources << /Font << /F1 7 0 R >> /ExtGState {at} 0 R >>");
                self.resources_one = Some(resources.clone());
                self.resources_two = Some(resources);
            }
            Sharing::Entry => {}
        }
        self
    }

    fn pdf(&self) -> Vec<u8> {
        let page = |resources: &Option<String>, content: u32| {
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] {} /Contents {content} 0 R >>",
                resources.as_deref().unwrap_or(OWN_RESOURCES)
            )
        };
        let mut objects: Vec<Written> = vec![
            (1, 0, "<< /Type /Catalog /Pages 2 0 R >>".to_owned()),
            (
                2,
                0,
                self.pages.clone().unwrap_or_else(|| {
                    "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned()
                }),
            ),
            (3, 0, page(&self.resources_one, 5)),
            (4, 0, page(&self.resources_two, 6)),
            (5, 0, stream("", self.page_one)),
            (6, 0, stream("", self.page_two)),
            (7, 0, HELVETICA.to_owned()),
            (8, 0, self.state.to_owned()),
        ];
        for (at, body) in self.extra.iter().enumerate() {
            let number = u32::try_from(9 + at).expect("a small object number");
            objects.push((number, 0, body.clone()));
        }
        classic(&objects)
    }
}

/// Every content-carrying kind the allowlist refuses, with what each reaches.
fn content_carrying_kinds() -> Vec<(&'static str, &'static str, Vec<String>)> {
    let type_three = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix \
                      [0.001 0 0 0.001 0 0] /CharProcs << /a 10 0 R >> /Encoding << /Type \
                      /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths \
                      [1000] >>";
    vec![
        (
            "a soft mask",
            "<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 9 0 R >> >>",
            vec![mask_form()],
        ),
        ("a transfer function", "<< /Type /ExtGState /TR 9 0 R >>", vec![sampled()]),
        ("a second transfer function", "<< /Type /ExtGState /TR2 9 0 R >>", vec![sampled()]),
        ("a black generation", "<< /Type /ExtGState /BG 9 0 R >>", vec![sampled()]),
        ("a second black generation", "<< /Type /ExtGState /BG2 9 0 R >>", vec![sampled()]),
        ("an undercolour removal", "<< /Type /ExtGState /UCR 9 0 R >>", vec![sampled()]),
        ("a second undercolour removal", "<< /Type /ExtGState /UCR2 9 0 R >>", vec![sampled()]),
        (
            "a halftone",
            "<< /Type /ExtGState /HT 9 0 R >>",
            vec![stream("/Type /Halftone /HalftoneType 6 /Width 4 /Height 4", "0123456789ABCDEF")],
        ),
        (
            "a Type 3 font",
            "<< /Type /ExtGState /Font [9 0 R 24] >>",
            vec![type_three.to_owned(), stream("", "1000 0 0 0 1000 1000 d0 0 0 1000 1000 re f")],
        ),
        (
            "an embedded font",
            "<< /Type /ExtGState /Font [9 0 R 24] >>",
            vec![
                "<< /Type /Font /Subtype /Type1 /BaseFont /Embedded /FirstChar 32 /LastChar 32 \
                 /Widths [500] /FontDescriptor 10 0 R >>"
                    .to_owned(),
                "<< /Type /FontDescriptor /FontName /Embedded /Flags 32 /FontBBox [0 0 1000 1000] \
                 /ItalicAngle 0 /Ascent 1000 /Descent 0 /CapHeight 1000 /StemV 80 /FontFile 11 0 R >>"
                    .to_owned(),
                stream("/Length1 8 /Length2 0 /Length3 0", "%!FONTFI"),
            ],
        ),
    ]
}

/// THE RULE, over every kind and every way two pages share a graphics state: an entry the part's
/// pages do not draw with that could carry content refuses -- and the part of page 2, which draws
/// with it, splits. The control twin, where page 1 does not list `/GSX`, splits too.
#[test]
fn an_unused_content_carrying_graphics_state_refuses_however_it_is_shared() {
    let mut refused = 0;
    for (kind, state, extra) in content_carrying_kinds() {
        for sharing in [Sharing::Inherited, Sharing::Category, Sharing::Entry] {
            let pdf = Two::new(state, extra.clone()).sharing(sharing).pdf();
            refuses(&format!("{kind}, {sharing:?}"), &pdf, 0, 1, UNUSED);
            refused += 1;
            splits(&format!("{kind}, {sharing:?}, page 2 alone"), &pdf, 1, 1);
        }
        let mut control = Two::new(state, extra.clone());
        control.resources_one = Some(FONT_ONLY.to_owned());
        splits(&format!("{kind}, control"), &control.pdf(), 0, 1);
    }
    assert_eq!(refused, 30, "ten kinds over three sharing shapes");
}

/// NO REACHABILITY IS COMPUTED, so a graphics state that shares nothing with page 2's still
/// refuses when page 1 lists it and does not draw with it -- one level down, two, or three.
#[test]
fn an_unused_content_carrying_graphics_state_refuses_whatever_it_shares_and_however_deep() {
    let own_mask = "<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 9 0 R >> >>";
    let mut nothing_shared = Two::new(own_mask, vec![mask_form()]);
    nothing_shared.resources_two = Some(FONT_ONLY.to_owned());
    nothing_shared.page_two = "BT /F1 24 Tf 20 200 Td (PAGETWO) Tj ET";
    refuses(
        "page 1's own mask, unused",
        &nothing_shared.pdf(),
        0,
        1,
        UNUSED,
    );

    // Each page's own direct graphics state, each naming the same mask form.
    let mut direct = Two::new("<< >>", vec![mask_form()]);
    let state = "/Resources << /Font << /F1 7 0 R >> /ExtGState << /GSX << /Type /ExtGState \
                 /SMask << /Type /Mask /S /Luminosity /G 9 0 R >> >> >> >>";
    direct.resources_one = Some(state.to_owned());
    direct.resources_two = Some(state.to_owned());
    refuses(
        "own direct states naming one form",
        &direct.pdf(),
        0,
        1,
        UNUSED,
    );

    // Two levels: each page's own state names its own indirect mask, both masks one form.
    let mut two_levels = Two::new(
        "<< >>",
        vec![mask_form(), mask_dictionary(9), mask_dictionary(9)],
    );
    two_levels.resources_one = Some(own_state(10));
    two_levels.resources_two = Some(own_state(11));
    refuses("two levels to one form", &two_levels.pdf(), 0, 1, UNUSED);

    // Three: the mask's group form draws a form page 2's mask also reaches.
    let wrapper = stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Group << /S /Transparency /CS \
         /DeviceGray >> /Resources << /XObject << /Fx 9 0 R >> >>",
        "/Fx Do",
    );
    let three = Two::new(
        "<< /Type /ExtGState /SMask 11 0 R >>",
        vec![mask_form(), wrapper, mask_dictionary(10)],
    );
    refuses("three levels to one form", &three.pdf(), 0, 1, UNUSED);
}

fn mask_dictionary(form: u32) -> String {
    format!("<< /Type /Mask /S /Luminosity /G {form} 0 R >>")
}

fn own_state(mask: u32) -> String {
    format!(
        "/Resources << /Font << /F1 7 0 R >> /ExtGState << /GSX << /Type /ExtGState /SMask \
         {mask} 0 R >> >> >>"
    )
}

/// THE ALLOWLIST'S EDGES, each value next to the near-miss that refuses. An unused entry travels
/// only when every key in it is listed with a direct value of its listed shape.
#[test]
fn each_allowlisted_key_accepts_its_shape_and_refuses_the_near_miss() {
    let cases: &[(&str, &str, &str, &[String])] = &[
        ("/SMask", "/SMask /None", "/SMask /Foo", &[]),
        (
            "/SMask behind a reference",
            "/SMask /None",
            "/SMask 9 0 R",
            &["/None".to_owned()],
        ),
        (
            "/ca behind a reference",
            "/ca 0",
            "/ca 9 0 R",
            &["0".to_owned()],
        ),
        ("/LW", "/LW 2.5", "/LW (TEXT)", &[]),
        ("/OP", "/OP true /op false", "/OP 1", &[]),
        ("/Type", "/Type /ExtGState", "/Type /Other", &[]),
        ("/RI", "/RI /Perceptual", "/RI /Secret", &[]),
        ("/BM", "/BM /Multiply", "/BM /Secret", &[]),
        (
            "/BM as an array",
            "/BM [ /Multiply /Normal ]",
            "/BM [ /Normal /Secret ]",
            &[],
        ),
        (
            "/D",
            "/D [ [ 3 2 ] 0 ]",
            "/D 9 0 R",
            &["[ [ 3 2 ] 0 ]".to_owned()],
        ),
        (
            "/D's dashes",
            "/D [ [ 3 2 ] 0 ]",
            "/D [ [ 9 0 R ] 0 ]",
            &["3".to_owned()],
        ),
        (
            "/D's phase",
            "/D [ [ 3 2 ] 0 ]",
            "/D [ [ 3 2 ] /Phase ]",
            &[],
        ),
        // Round 1 (code SF2, security F-2): each row below is the one input a surviving mutation
        // let through -- a third item, a third item behind a reference, the dashes or the phase
        // behind a reference, and a blend mode behind a reference.
        (
            "/D with a third item",
            "/D [ [ 3 2 ] 0 ]",
            "/D [ [ 3 2 ] 0 1 ]",
            &[],
        ),
        (
            "/D with a third item behind a reference",
            "/D [ [ 3 2 ] 0 ]",
            "/D [ [ 3 2 ] 0 9 0 R ]",
            &["(TEXT)".to_owned()],
        ),
        (
            "/D's dash array behind a reference",
            "/D [ [ 3 2 ] 0 ]",
            "/D [ 9 0 R 0 ]",
            &["[ 3 2 ]".to_owned()],
        ),
        (
            "/D's phase behind a reference",
            "/D [ [ 3 2 ] 0 ]",
            "/D [ [ 3 2 ] 9 0 R ]",
            &["0".to_owned()],
        ),
        (
            "/BM's array item behind a reference",
            "/BM [ /Multiply ]",
            "/BM [ 9 0 R ]",
            &["/Multiply".to_owned()],
        ),
        ("/TR", "/TR /Identity", "/TR /Default", &[]),
        ("/TR2", "/TR2 /Default", "/TR2 /Other", &[]),
        ("/BG2", "/BG2 /Default", "/BG2 /Identity", &[]),
        ("/UCR2", "/UCR2 /Default", "/UCR2 /Identity", &[]),
        ("/HT", "/HT /Default", "/HT /Identity", &[]),
        (
            "a key outside the allowlist",
            "/ca 0.5",
            "/UseBlackPtComp /ON",
            &[],
        ),
        ("a key carrying text", "/ca 0.5", "/Secret (TEXT)", &[]),
    ];
    for (what, accepted, refused, extra) in cases {
        let state = format!("<< {accepted} >>");
        splits(
            &format!("{what}: {accepted}"),
            &Two::new(&state, extra.to_vec()).pdf(),
            0,
            1,
        );
        let state = format!("<< {refused} >>");
        refuses(
            &format!("{what}: {refused}"),
            &Two::new(&state, extra.to_vec()).pdf(),
            0,
            1,
            UNUSED,
        );
    }
}

/// A graphics-state entry that is not a dictionary is not one the allowlist can clear.
#[test]
fn an_unused_graphics_state_that_is_not_a_dictionary_refuses() {
    for (what, state) in [
        ("a stream", stream("/Type /ExtGState /ca 0", "STREAMDATA")),
        ("an array", "[ /ca 0 ]".to_owned()),
        ("a number", "0".to_owned()),
    ] {
        refuses(what, &Two::new(&state, Vec::new()).pdf(), 0, 1, UNUSED);
    }
}

/// THE KEEP. An unused content-free graphics state travels into the part, whole -- where every
/// other category's unused entry is pruned. The Distiller defaults ride on the same rule.
#[test]
fn an_unused_content_free_graphics_state_is_kept_whole() {
    for (what, state) in [
        (
            "every allowlisted key",
            "<< /Type /ExtGState /ca 0.5 /CA 0.5 /LW 2 /LC 1 /LJ 1 /ML 10 /D [ [ 3 2 ] 0 ] /RI \
             /Perceptual /OP false /op true /OPM 1 /SA true /SM 0.02 /FL 1 /AIS false /TK true \
             /BM [ /Multiply /Normal ] /SMask /None >>",
        ),
        (
            "the spec's default names",
            "<< /Type /ExtGState /TR /Identity /TR2 /Default /BG2 /Default /UCR2 /Default /HT \
             /Default >>",
        ),
        ("an empty one", "<< >>"),
    ] {
        let part = splits(what, &Two::new(state, Vec::new()).pdf(), 0, 1);
        assert!(
            contains(&part, "/GSX"),
            "{what}: the unused state was pruned off the part"
        );
    }
}

/// What the part's pages draw with is theirs, content and all: a page's own soft mask, transfer,
/// halftone and font, and a soft mask both pages share and draw with.
#[test]
fn a_content_carrying_graphics_state_the_part_draws_with_travels() {
    for (kind, state, extra) in content_carrying_kinds() {
        let mut own = Two::new(state, extra.clone());
        own.page_one = PAGE_ONE_USES;
        own.resources_two = Some(FONT_ONLY.to_owned());
        own.page_two = "BT /F1 24 Tf 20 200 Td (PAGETWO) Tj ET";
        splits(&format!("{kind}, page 1's own, drawn"), &own.pdf(), 0, 1);

        let mut shared = Two::new(state, extra);
        shared.page_one = PAGE_ONE_USES;
        splits(
            &format!("{kind}, shared, drawn by both"),
            &shared.pdf(),
            0,
            1,
        );
    }
}

/// POOLED BY THE DICTIONARY. Two pages with their own `/Resources` share one `/ExtGState`; page 1
/// draws `/GA`, page 2 `/GB`, both soft masks. A part of both pages uses both, so it splits; a
/// part of page 1 alone does not use `/GB`, so it refuses. The same holds where the shared thing is
/// the `/Resources` holding a direct `/ExtGState`.
#[test]
fn a_shared_graphics_state_dictionary_is_judged_by_every_kept_page_that_shares_it() {
    let mask = "<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 9 0 R >> >>";
    let resources = "/Resources << /Font << /F1 7 0 R >> /ExtGState 11 0 R >>";
    let mut union = Two::new(
        mask,
        vec![
            mask_form(),
            mask.to_owned(),
            "<< /GA 8 0 R /GB 10 0 R >>".to_owned(),
        ],
    );
    union.page_one = "q /GA gs 0 0 300 400 re f Q";
    union.page_two = "q /GB gs 0 0 300 400 re f Q";
    union.resources_one = Some(resources.to_owned());
    union.resources_two = Some(resources.to_owned());
    let pdf = union.pdf();
    let both = splits("both pages", &pdf, 0, 2);
    assert!(
        contains(&both, "/GA") && contains(&both, "/GB"),
        "a used entry was pruned"
    );
    refuses("page 1 alone", &pdf, 0, 1, UNUSED);

    // The same through one indirect `/Resources` both pages share, holding a direct `/ExtGState`:
    // pooled by the `/Resources`.
    let shared = "/Resources 11 0 R";
    let mut holder = Two::new(
        mask,
        vec![
            mask_form(),
            mask.to_owned(),
            "<< /Font << /F1 7 0 R >> /ExtGState << /GA 8 0 R /GB 10 0 R >> >>".to_owned(),
        ],
    );
    holder.page_one = "q /GA gs 0 0 300 400 re f Q";
    holder.page_two = "q /GB gs 0 0 300 400 re f Q";
    holder.resources_one = Some(shared.to_owned());
    holder.resources_two = Some(shared.to_owned());
    let pdf = holder.pdf();
    splits("both pages, one /Resources", &pdf, 0, 2);
    refuses("page 1 alone, one /Resources", &pdf, 0, 1, UNUSED);
}

/// A DIRECT `/ExtGState` IN A DIRECT `/Resources` IS ITS PAGE'S ALONE (round 1, security F-2).
/// Three pages, every `/Resources` and `/ExtGState` written directly; the part is pages 1 and 2.
/// Page 1 lists `/GSX`, the soft mask page 3 draws with, and does not draw with it; page 2 lists a
/// content-free `/GSX` of its own and draws with it. The two direct dictionaries are not one pool,
/// so page 2's use does not clear page 1's entry. The twin: page 1 draws with it too.
#[test]
fn direct_graphics_state_dictionaries_on_different_pages_are_not_pooled() {
    let pdf = |page_one: &str| {
        let page = |states: &str, content: u32| {
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /Font << \
                 /F1 7 0 R >> /ExtGState << /GSX {states} >> >> /Contents {content} 0 R >>"
            )
        };
        let objects: Vec<Written> = vec![
            (1, 0, "<< /Type /Catalog /Pages 2 0 R >>".to_owned()),
            (
                2,
                0,
                "<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R 5 0 R] >>".to_owned(),
            ),
            (3, 0, page("9 0 R", 10)),
            (4, 0, page("<< /ca 0.5 >>", 11)),
            (5, 0, page("9 0 R", 12)),
            (7, 0, HELVETICA.to_owned()),
            (8, 0, mask_form()),
            (
                9,
                0,
                "<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 8 0 R >> >>"
                    .to_owned(),
            ),
            (10, 0, stream("", page_one)),
            (11, 0, stream("", PAGE_ONE_USES)),
            (12, 0, stream("", PAGE_TWO)),
        ];
        classic(&objects)
    };
    refuses(
        "page 1 lists the mask and does not draw with it",
        &pdf(PAGE_ONE),
        0,
        2,
        UNUSED,
    );
    splits("page 1 draws with the mask", &pdf(PAGE_ONE_USES), 0, 2);
}

/// A deep page tree: the resources two `/Pages` levels up, page 1 with its own `/Rotate`. The copy
/// moves them onto the page, and the rule applies as to any other.
#[test]
fn resources_inherited_two_levels_up_are_judged_like_the_pages_own() {
    let tree = |state: &str, extra: &[String]| {
        let mut objects: Vec<Written> = vec![
            (1, 0, "<< /Type /Catalog /Pages 2 0 R >>".to_owned()),
            (2, 0, format!("<< /Type /Pages /Count 2 /Kids [11 0 R] {OWN_RESOURCES} >>")),
            (
                3,
                0,
                "<< /Type /Page /Parent 12 0 R /Rotate 90 /MediaBox [0 0 300 400] /Contents 5 0 R >>"
                    .to_owned(),
            ),
            (
                4,
                0,
                "<< /Type /Page /Parent 12 0 R /MediaBox [0 0 300 400] /Contents 6 0 R >>".to_owned(),
            ),
            (5, 0, stream("", PAGE_ONE)),
            (6, 0, stream("", PAGE_TWO)),
            (7, 0, HELVETICA.to_owned()),
            (8, 0, state.to_owned()),
            (9, 0, extra.first().cloned().unwrap_or_else(|| "<< >>".to_owned())),
            (11, 0, "<< /Type /Pages /Count 2 /Parent 2 0 R /Kids [12 0 R] >>".to_owned()),
            (12, 0, "<< /Type /Pages /Count 2 /Parent 11 0 R /Kids [3 0 R 4 0 R] >>".to_owned()),
        ];
        objects.sort_by_key(|(number, _, _)| *number);
        classic(&objects)
    };
    refuses(
        "an unused soft mask two levels up",
        &tree(
            "<< /Type /ExtGState /SMask << /Type /Mask /S /Luminosity /G 9 0 R >> >>",
            &[mask_form()],
        ),
        0,
        1,
        UNUSED,
    );
    let part = splits(
        "a content-free state two levels up",
        &tree("<< /ca 0.5 >>", &[]),
        0,
        1,
    );
    assert!(
        contains(&part, "/Rotate 90"),
        "page 1's own /Rotate did not travel"
    );
    assert!(contains(&part, "/GSX"), "the inherited state was not kept");
}

/// EVERY RESOURCE CATEGORY written as something other than a dictionary refuses: pruning cannot
/// filter it, and readers resolve resources from it. A dictionary, or an absent category, splits
/// (a `null` one is refused before this, `[split-null-value]`).
#[test]
fn a_resource_category_that_is_not_a_dictionary_refuses() {
    let one_page = |resources: &str, extra: Option<String>| {
        let mut objects: Vec<Written> = vec![
            (1, 0, "<< /Type /Catalog /Pages 2 0 R >>".to_owned()),
            (2, 0, "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned()),
            (
                3,
                0,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /Font \
                     << /F1 5 0 R >> {resources} >> /Contents 4 0 R >>"
                ),
            ),
            (4, 0, stream("", PAGE_ONE)),
            (5, 0, HELVETICA.to_owned()),
        ];
        if let Some(body) = extra {
            objects.push((6, 0, body));
        }
        classic(&objects)
    };
    let mut refused = 0;
    for category in [
        "XObject",
        "Pattern",
        "Shading",
        "ColorSpace",
        "ExtGState",
        "Properties",
    ] {
        for (what, body) in [
            ("a stream", stream("", "")),
            ("an array", "[ ]".to_owned()),
            ("a number", "0".to_owned()),
        ] {
            refuses(
                &format!("/{category} as {what}"),
                &one_page(&format!("/{category} 6 0 R"), Some(body)),
                0,
                1,
                NOT_A_DICTIONARY,
            );
            refused += 1;
        }
        splits(
            &format!("/{category} as a dictionary"),
            &one_page(&format!("/{category} 6 0 R"), Some("<< >>".to_owned())),
            0,
            1,
        );
    }
    // Absent: the page's own fonts, and nothing else.
    splits("no other category", &one_page("", None), 0, 1);
    // `/Font` itself, which the page needs as a dictionary to draw at all.
    let font_stream = classic(&[
        (1, 0, "<< /Type /Catalog /Pages 2 0 R >>".to_owned()),
        (2, 0, "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned()),
        (
            3,
            0,
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << /Font 5 0 R >> \
             /Contents 4 0 R >>"
                .to_owned(),
        ),
        (4, 0, stream("", PAGE_ONE)),
        (5, 0, stream("/F1 6 0 R", "")),
        (6, 0, HELVETICA.to_owned()),
    ]);
    refuses("/Font as a stream", &font_stream, 0, 1, NOT_A_DICTIONARY);
    refused += 1;
    assert_eq!(refused, 19, "six categories in three shapes, and /Font");
}

/// THE CLIMB, on a page whose inherited `/Resources` the copy has not moved: asked of a source
/// document, where the attribute is still on the page tree. It refuses, and a page tree deeper
/// than the climb refuses at the ceiling. A page with nothing to inherit passes.
#[test]
fn a_page_inheriting_resources_refuses_and_the_climb_refuses_at_its_ceiling() {
    use super::open_document;
    use super::prune::QpdfGraph;
    use crate::prune::graph::ObjectGraph;

    let check = |pdf: Vec<u8>| -> Result<(), String> {
        let (document, _, _, deadline) =
            open_document(pdf.into_boxed_slice(), &options()).expect("the fixture opens");
        let graph = QpdfGraph::over(&document);
        let page = graph.page(0).expect("page 0");
        let clock = ManualClock::new(0);
        crate::prune::refuse_inherited_resources(&graph, &page, &deadline, &clock)
            .map_err(|error| error.to_string())
    };
    let chain = |depth: u32, resources: &str| {
        // Object 1 the catalog, 2 the page, 3 its content, 10.. the /Pages chain, 10 the root.
        let mut objects: Vec<Written> = vec![
            (1, 0, "<< /Type /Catalog /Pages 10 0 R >>".to_owned()),
            (
                2,
                0,
                format!(
                    "<< /Type /Page /Parent {} 0 R /MediaBox [0 0 300 400] /Contents 3 0 R >>",
                    10 + depth - 1
                ),
            ),
            (3, 0, stream("", "")),
        ];
        for level in 0..depth {
            let number = 10 + level;
            let kids = if level + 1 == depth {
                "2 0 R".to_owned()
            } else {
                format!("{} 0 R", number + 1)
            };
            let parent = if level == 0 {
                format!(" {resources}")
            } else {
                format!(" /Parent {} 0 R", number - 1)
            };
            objects.push((
                number,
                0,
                format!("<< /Type /Pages /Count 1 /Kids [{kids}]{parent} >>"),
            ));
        }
        classic(&objects)
    };
    let inherited = check(chain(3, "/Resources << >>")).expect_err("inherits");
    assert!(
        inherited.contains("[split-resources-inherited]"),
        "{inherited}"
    );
    check(chain(3, "")).expect("nothing to inherit");
    let deep = check(chain(70, "")).expect_err("deeper than the climb");
    assert!(deep.contains("[split-page-tree-too-deep]"), "{deep}");
    check(chain(60, "")).expect("inside the climb");
    // THE BOUNDARY. A page under 63 `/Pages` levels takes 64 steps to find the root's absent
    // `/Parent`, the ceiling exactly; one level more refuses.
    check(chain(63, "")).expect("at the ceiling");
    let over = check(chain(64, "")).expect_err("one past the ceiling");
    assert!(over.contains("[split-page-tree-too-deep]"), "{over}");
}

/// THE DEADLINE IS READ AT EVERY ENTRY AND EVERY ARRAY ITEM. A page listing many graphics states it
/// does not draw with, or one whose dash array is long, is inside `max_duration_ms`. Counted by
/// difference against the same page without them, so the walk's own reads do not count. Each pass
/// over the entries reads it, so a mutation deleting any one of the three reads fails here.
#[test]
fn the_graphics_state_check_reads_the_deadline_per_entry_and_per_item() {
    struct Counting(std::sync::atomic::AtomicU64);
    impl Clock for Counting {
        fn now_ms(&self) -> u64 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        }
    }
    let reads = |entries: &str| {
        let pdf = classic(&[
            (1, 0, "<< /Type /Catalog /Pages 2 0 R >>".to_owned()),
            (2, 0, "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned()),
            (
                3,
                0,
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << \
                     /ExtGState << {entries} >> >> /Contents 4 0 R >>"
                ),
            ),
            (4, 0, stream("", "0 0 1 1 re f")),
        ]);
        let clock = Arc::new(Counting(std::sync::atomic::AtomicU64::new(0)));
        let shared: Arc<dyn Clock> = clock.clone();
        part_on(&super::Qpdf::new(), &pdf, 0, 1, &options_with(shared)).expect("splits");
        clock.0.load(std::sync::atomic::Ordering::Relaxed)
    };
    let base = reads("");
    let entries: String = (0..40).map(|at| format!("/G{at} << >> ")).collect();
    let many = reads(&entries);
    assert!(
        many >= base + 3 * 40,
        "{many} reads with 40 unused entries, {base} without: each of the three passes over them \
         -- recording the dictionary, striking what a page draws with, judging what is left -- \
         must read the deadline at each"
    );
    let dashes: String = (0..100).map(|_| "1 ").collect();
    let long = reads(&format!("/G0 << /D [ [ {dashes}] 0 ] >>"));
    let short = reads("/G0 << /D [ [ ] 0 ] >>");
    assert!(
        long >= short + 100,
        "{long} reads over a 100-item dash array, {short} over an empty one: the check must read \
         the deadline at each item"
    );
}
