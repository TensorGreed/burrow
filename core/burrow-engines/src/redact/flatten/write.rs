//! The flatten write and its read-back: see the parent module for the steps and their limits.

use core::ffi::c_int;
use std::ffi::CString;
use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use super::super::frames::{PageFrameReading, PageFrames};
use super::super::graph::{BuildsObjects, OpensForRedaction, PdfDocument, PdfObject};
use crate::codes::qpdf::object_type;
use crate::name::Name;

use super::{MAX_FLATTEN_PIXELS, Picture, pixels_for};

/// The name the image is drawn by, in `/Resources /XObject` and in the content line.
const IMAGE: Name = Name::literal(b"/Im0\0");

/// The content line, for a page of `width` × `height` points: the image scaled to the frame,
/// from the origin. Fixed text but for the two sides (§1), which V2 compares byte for byte.
fn content_line(width: &str, height: &str) -> Vec<u8> {
    format!("q {width} 0 0 {height} 0 0 cm /Im0 Do Q\n").into_bytes()
}

/// A frame side as the real qpdf is handed: Rust's shortest round-trip spelling of the `f64`.
///
/// # Errors
///
/// [`Error::Internal`] unless the side is finite and positive and the text is digits with at most
/// one point that parses back to exactly the same `f64`. Every one of those is true of a frame
/// `redact::frames` accepts; the check makes "qpdf writes back the value the frame was" a
/// measurement here rather than a property of `Display` this file assumes.
fn real_text(side: f64) -> Result<String> {
    let text = format!("{side}");
    let well_formed = side.is_finite()
        && side > 0.0
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && text.bytes().filter(|byte| *byte == b'.').count() <= 1
        && text.parse::<f64>().ok().map(f64::to_bits) == Some(side.to_bits());
    if well_formed {
        Ok(text)
    } else {
        Err(Error::Internal(
            "pdf flatten: a frame side with no exact decimal spelling".to_owned(),
        ))
    }
}

const TYPE: Name = Name::literal(b"/Type\0");
const PARENT: Name = Name::literal(b"/Parent\0");
const ROTATE: Name = Name::literal(b"/Rotate\0");
const MEDIA_BOX: Name = Name::literal(b"/MediaBox\0");
const CROP_BOX: Name = Name::literal(b"/CropBox\0");
const RESOURCES: Name = Name::literal(b"/Resources\0");
const CONTENTS: Name = Name::literal(b"/Contents\0");
const XOBJECT: Name = Name::literal(b"/XObject\0");
const SUBTYPE: Name = Name::literal(b"/Subtype\0");
const WIDTH: Name = Name::literal(b"/Width\0");
const HEIGHT: Name = Name::literal(b"/Height\0");
const COLOR_SPACE: Name = Name::literal(b"/ColorSpace\0");
const BITS_PER_COMPONENT: Name = Name::literal(b"/BitsPerComponent\0");
const FILTER: Name = Name::literal(b"/Filter\0");
const ROOT: Name = Name::literal(b"/Root\0");
const STRUCT_TREE_ROOT: Name = Name::literal(b"/StructTreeRoot\0");
const MARK_INFO: Name = Name::literal(b"/MarkInfo\0");
const PAGE_VALUE: Name = Name::literal(b"/Page\0");
const IMAGE_VALUE: Name = Name::literal(b"/Image\0");
const DEVICE_RGB: Name = Name::literal(b"/DeviceRGB\0");

/// §3's keys: the flattened page's dictionary holds these and nothing else (V2). Sorted.
pub(crate) const PAGE_KEYS: [&[u8]; 7] = [
    b"Contents",
    b"CropBox",
    b"MediaBox",
    b"Parent",
    b"Resources",
    b"Rotate",
    b"Type",
];

/// The image dictionary's keys after qpdf's write (V3), sorted: §1's, plus the `/Filter` and
/// `/Length` qpdf adds as it encodes the stream. No `/DecodeParms`, `/Decode`, `/SMask`, `/Mask`,
/// `/Interpolate`, `/Intent` or `/OC`.
pub(crate) const IMAGE_KEYS: [&[u8]; 8] = [
    b"BitsPerComponent",
    b"ColorSpace",
    b"Filter",
    b"Height",
    b"Length",
    b"Subtype",
    b"Type",
    b"Width",
];

/// What the flattened page must read back as: computed before the edit, from the input.
#[derive(Debug)]
struct Promise {
    page: usize,
    /// Every page's frame reading, from the input, with page *N*'s the display frame.
    frames: Vec<PageFrameReading>,
    width_text: String,
    height_text: String,
    pixels_wide: u32,
    pixels_high: u32,
}

/// Replace page `page` of `bytes` with `picture`, write the document, and return it only if it
/// reads back as `check_output` requires.
///
/// `frames` reads every page's frame of a document the way redaction reads it
/// (`crate::PageFrameReader::page_frames`); it is asked of the input and then of the output.
///
/// # Errors
///
/// - Whatever the redaction's open refuses, through `frames` and [`OpensForRedaction`].
/// - Page `page`'s own frame refusal, if it has one: a page with no frame has nothing to flatten
///   to. [`Error::InvalidArgument`] for a page past the end.
/// - [`Error::LimitExceeded`] for `max_pixels` or [`MAX_FLATTEN_PIXELS`] on *W*·*H*, before
///   anything is allocated for the picture; `max_duration_ms`, checkpointed between steps.
/// - [`Error::Internal`] for a picture whose size is not the frame's at 150 dpi, or whose buffer
///   is not exactly *W*·*H*·3 bytes: the drawing side is burrow's, so either is a bug.
/// - `[engine-repaired-input]` for a repair at the open's resolution, after the constructors, or
///   in the write.
/// - [`Error::OutputRejected`] for an output that does not read back as promised.
pub(crate) fn flatten<E, F>(
    engine: &E,
    bytes: &[u8],
    page: usize,
    picture: &Picture<'_>,
    options: &crate::OpenOptions<'_>,
    frames: F,
) -> Result<Vec<u8>>
where
    E: OpensForRedaction,
    for<'d> <E::Document as PdfDocument>::Object<'d>: BuildsObjects,
    F: Fn(&[u8]) -> Result<PageFrames>,
{
    let clock = Arc::clone(&options.clock);
    let deadline = Deadline::start(clock.as_ref(), &options.limits);

    // 1. THE FRAMES, FROM THE INPUT, before anything is edited.
    let promise = promise(frames(bytes)?, page, picture, &options.limits)?;
    deadline.checkpoint(clock.as_ref())?;

    // 3. THE OPEN, AND THE RESOLUTION INSIDE THE TRAP.
    let (document, _) = engine.open_for_redaction(bytes, options)?;
    let trailer = document.trailer()?;
    let resolved = trailer.null_beside().made_indirect()?;
    if document.repaired() {
        return Err(super::super::repaired_by_the_engine());
    }
    deadline.checkpoint(clock.as_ref())?;

    // 4. THE REPLACEMENT.
    let handle = document.page(page)?;
    same_frame(&handle, &promise)?;
    replace(&handle, &promise, picture)?;
    drop_structure(&trailer)?;
    #[cfg(test)]
    lie::apply(&document, &handle, &promise)?;

    // THE CONSTRUCTORS' ARGUMENT, CHECKED: a repair here means a resolution happened outside the
    // trap, which the open's forced resolution exists to prevent.
    if document.repaired() {
        return Err(super::super::repaired_by_the_engine());
    }
    deadline.checkpoint(clock.as_ref())?;

    let output = document.write()?;
    if document.repaired() {
        return Err(super::super::repaired_by_the_engine());
    }
    // EVERY HANDLE, AND THE DOCUMENT, released before the read-back opens its own.
    drop((handle, trailer, resolved));
    drop(document);
    deadline.checkpoint(clock.as_ref())?;

    // 5. THE READ-BACK, through fresh opens of the emitted bytes.
    check_output(engine, &output, &promise, picture, options, &frames)?;
    deadline.checkpoint(clock.as_ref())?;
    Ok(output)
}

/// Read the input's frames into what the output must hold, and hold the picture to page *N*'s.
fn promise(
    read: PageFrames,
    page: usize,
    picture: &Picture<'_>,
    limits: &Limits,
) -> Result<Promise> {
    let mut frames = read.readings;
    let reading = frames.get_mut(page).ok_or_else(|| {
        Error::InvalidArgument("pdf flatten: a page index past the end of the document".to_owned())
    })?;
    let (width, height) = match reading {
        PageFrameReading::Frame { width, height } => (*width, *height),
        // THE PAGE'S OWN REFUSAL, as the redaction raises it: taken out, a placeholder left, and
        // the promise is never built.
        PageFrameReading::Refused { error, .. } => {
            return Err(core::mem::replace(error, Error::Internal(String::new())));
        }
    };
    let (pixels_wide, pixels_high) = (pixels_for(width)?, pixels_for(height)?);
    // RULE 6'S CEILINGS, on the product, exactly, before a byte is allocated for the picture.
    let pixels = u64::from(pixels_wide) * u64::from(pixels_high);
    Limits::check(Stage::Pixels, "max_pixels", pixels, limits.max_pixels)?;
    Limits::check(
        Stage::Pixels,
        "redaction_flatten_pixels",
        pixels,
        MAX_FLATTEN_PIXELS,
    )?;
    // §1 STEP 6'S SIZE AND LENGTH: the drawing side's size against the frame's, and the buffer
    // against the size, by checked arithmetic.
    if picture.width != pixels_wide || picture.height != pixels_high {
        return Err(Error::Internal(
            "pdf flatten: the picture is not the page's display frame at 150 dpi".to_owned(),
        ));
    }
    let length = pixels
        .checked_mul(3)
        .and_then(|bytes| usize::try_from(bytes).ok());
    if length != Some(picture.rgb.len()) {
        return Err(Error::Internal(
            "pdf flatten: the picture's buffer is not three bytes per pixel".to_owned(),
        ));
    }
    Ok(Promise {
        page,
        width_text: real_text(width)?,
        height_text: real_text(height)?,
        frames,
        pixels_wide,
        pixels_high,
    })
}

/// The page about to be replaced has the frame the promise was computed from.
///
/// The promise was read through another open of the same bytes. qpdf reads them alike, and a
/// repair in either is refused, so this should never fire; it is the cheap way to make "the page
/// replaced is the page measured" a check rather than an inference.
fn same_frame<O: PdfObject>(page: &O, promise: &Promise) -> Result<()> {
    let frame = super::super::frame::of(page)?;
    let (width, height) = frame.displayed_size();
    match promise.frames.get(promise.page) {
        Some(PageFrameReading::Frame {
            width: promised_width,
            height: promised_height,
        }) if promised_width.to_bits() == width.to_bits()
            && promised_height.to_bits() == height.to_bits() =>
        {
            Ok(())
        }
        _ => Err(Error::Internal(
            "pdf flatten: the page to replace reads differently from the page measured".to_owned(),
        )),
    }
}

/// An array `[0 0 width height]`, beside `near`.
fn frame_box<O: BuildsObjects>(near: &O, width: &str, height: &str) -> Result<O> {
    let array = near.array_beside();
    array.append_item(&near.integer_beside(0))?;
    array.append_item(&near.integer_beside(0))?;
    array.append_item(&near.real_beside(&c_text(width)?))?;
    array.append_item(&near.real_beside(&c_text(height)?))?;
    Ok(array)
}

fn c_text(text: &str) -> Result<CString> {
    CString::new(text)
        .map_err(|_| Error::Internal("pdf flatten: a number with a NUL in it".to_owned()))
}

/// §3: page *N*'s dictionary, in place, holding the picture and nothing else.
fn replace<O: BuildsObjects>(page: &O, promise: &Promise, picture: &Picture<'_>) -> Result<()> {
    // A PAGE WITH NO `/Parent` is not one this can write §3's seven keys for.
    if page.key(&PARENT).type_code() != object_type::DICTIONARY {
        return Err(Error::Malformed(
            "pdf flatten: the page has no /Parent dictionary".to_owned(),
        ));
    }
    // EVERY KEY BUT `/Parent` GOES, each read through `unparse` (qpdf's key iterator is untrapped).
    for key in crate::pdfsyntax::dict::top_level_keys(&page.unparse())? {
        if key.as_slice() == b"Parent" {
            continue;
        }
        page.remove_key(&Name::from_stripped(&key)?);
        page.drained()?;
    }

    // THE IMAGE: 8-bit `/DeviceRGB`, no filter of ours. qpdf encodes it with `/FlateDecode` in the
    // write, and V3 reads back that it did.
    let null = page.null_beside();
    let image = page.stream_beside();
    image.drained()?;
    image.replace_stream_data(picture.rgb, &null, &null)?;
    let header = image.stream_dict();
    header.replace_key(&TYPE, &page.name_beside(&XOBJECT))?;
    header.replace_key(&SUBTYPE, &page.name_beside(&IMAGE_VALUE))?;
    header.replace_key(&WIDTH, &page.integer_beside(i64::from(promise.pixels_wide)))?;
    header.replace_key(
        &HEIGHT,
        &page.integer_beside(i64::from(promise.pixels_high)),
    )?;
    header.replace_key(&COLOR_SPACE, &page.name_beside(&DEVICE_RGB))?;
    header.replace_key(&BITS_PER_COMPONENT, &page.integer_beside(8))?;

    let xobjects = page.dictionary_beside();
    xobjects.replace_key(&IMAGE, &image)?;
    let resources = page.dictionary_beside();
    resources.replace_key(&XOBJECT, &xobjects)?;

    let contents = page.stream_beside();
    contents.drained()?;
    contents.replace_stream_data(
        &content_line(&promise.width_text, &promise.height_text),
        &null,
        &null,
    )?;

    page.replace_key(&TYPE, &page.name_beside(&PAGE_VALUE))?;
    page.replace_key(&ROTATE, &page.integer_beside(0))?;
    page.replace_key(
        &MEDIA_BOX,
        &frame_box(page, &promise.width_text, &promise.height_text)?,
    )?;
    page.replace_key(
        &CROP_BOX,
        &frame_box(page, &promise.width_text, &promise.height_text)?,
    )?;
    page.replace_key(&RESOURCES, &resources)?;
    page.replace_key(&CONTENTS, &contents)?;
    Ok(())
}

/// §2 rule 1's drop: the catalogue loses `/StructTreeRoot` and `/MarkInfo`.
fn drop_structure<O: PdfObject>(trailer: &O) -> Result<()> {
    let root = trailer.key(&ROOT);
    if root.type_code() != object_type::DICTIONARY {
        return Err(Error::Malformed(
            "pdf flatten: the trailer's /Root is not a dictionary".to_owned(),
        ));
    }
    root.remove_key(&STRUCT_TREE_ROOT);
    root.remove_key(&MARK_INFO);
    root.drained()
}

/// The emitted document read back: V1's frames, V2 and V3 (ADR 0031 §4). Each item is re-derived
/// from `output` through fresh opens, never from the document that wrote it.
///
/// - **V1, frames:** every page's frame reading equals the input's; page *N*'s is its display
///   frame. (V1's page count and rotation vector are `burrow_ops::verify::output`'s.)
/// - **V2:** page *N* has exactly [`PAGE_KEYS`]; `/Type /Page`; `/Rotate 0`; `/MediaBox` and
///   `/CropBox` each exactly `[0 0 dw dh]` as written; `/Resources` exactly `/XObject` naming
///   exactly one stream; `/Contents` one stream whose decoded bytes are the line, byte for byte.
/// - **V4, the catalogue's half:** no `/StructTreeRoot` and no `/MarkInfo`.
/// - **V3:** the image has exactly [`IMAGE_KEYS`], `/Type /XObject`, `/Subtype /Image`, `/Width`
///   *W* and `/Height` *H* as derived here, `/DeviceRGB`, 8 bits, `/FlateDecode`; and its decoded
///   bytes are exactly the picture's, so exactly *W*·*H*·3 of them.
///
/// # What it leaves undetectable
///
/// Everything ADR 0031 §4 lists, and, until the later slices, V4's element walk and V5 to V7:
/// what else the output keeps from page *N* is not read here at all. The decoded read is capped
/// by qpdf's 256 MiB per-filter ceiling, not at *W*·*H*·3 + 1 bytes as V3 asks; the exact cap
/// needs a capped read the C API does not have, and is recorded for a later slice.
fn check_output<E, F>(
    engine: &E,
    output: &[u8],
    promise: &Promise,
    picture: &Picture<'_>,
    options: &crate::OpenOptions<'_>,
    frames: &F,
) -> Result<()>
where
    E: OpensForRedaction,
    F: Fn(&[u8]) -> Result<PageFrames>,
{
    // V1, FRAMES.
    let read = frames(output).map_err(|error| rejected("its frames cannot be read", &error))?;
    if read.readings.len() != promise.frames.len() {
        return Err(reject("a different number of pages came out"));
    }
    for (index, (before, after)) in promise.frames.iter().zip(&read.readings).enumerate() {
        if !same_reading(before, after) {
            return Err(Error::OutputRejected(format!(
                "pdf flatten: page {} of the output does not have the input's display frame",
                index + 1
            )));
        }
    }

    // V2 AND V3, on a fresh open.
    let (document, _) = engine
        .open_for_redaction(output, options)
        .map_err(|error| rejected("it cannot be read back", &error))?;
    let page = document
        .page(promise.page)
        .map_err(|error| rejected("its page cannot be read back", &error))?;

    // V4'S CATALOGUE HALF: neither key rule 1 drops. (Its element walk is a later slice.)
    let trailer = document
        .trailer()
        .map_err(|error| rejected("its trailer cannot be read back", &error))?;
    let root = trailer.key(&ROOT);
    let catalogue = crate::pdfsyntax::dict::top_level_keys(&root.unparse())
        .map_err(|error| rejected("its catalogue cannot be read back", &error))?;
    if catalogue
        .iter()
        .any(|key| key.as_slice() == b"StructTreeRoot" || key.as_slice() == b"MarkInfo")
    {
        return Err(reject(
            "the catalogue still has its structure tree or /MarkInfo",
        ));
    }

    keys_are(&page, &PAGE_KEYS, "the flattened page")?;
    name_is(&page.key(&TYPE), &PAGE_VALUE, "/Type")?;
    integer_is(&page.key(&ROTATE), 0, "/Rotate")?;
    box_is(&page.key(&MEDIA_BOX), promise, "/MediaBox")?;
    box_is(&page.key(&CROP_BOX), promise, "/CropBox")?;

    let resources = page.key(&RESOURCES);
    dictionary_keys_are(&resources, &[b"XObject"], "/Resources")?;
    let xobjects = resources.key(&XOBJECT);
    dictionary_keys_are(&xobjects, &[b"Im0"], "/Resources /XObject")?;

    let contents = page.key(&CONTENTS);
    if contents.type_code() != object_type::STREAM {
        return Err(reject("/Contents is not one stream"));
    }
    let line = contents
        .stream_data()
        .map_err(|error| rejected("its content cannot be decoded", &error))?;
    if line.as_deref() != Some(content_line(&promise.width_text, &promise.height_text).as_slice()) {
        return Err(reject("the content line is not the one written"));
    }

    // V3.
    let image = xobjects.key(&IMAGE);
    if image.type_code() != object_type::STREAM {
        return Err(reject("the image is not a stream"));
    }
    let header = image.stream_dict();
    keys_are(&header, &IMAGE_KEYS, "the image")?;
    name_is(&header.key(&TYPE), &XOBJECT, "the image's /Type")?;
    name_is(&header.key(&SUBTYPE), &IMAGE_VALUE, "the image's /Subtype")?;
    integer_is(
        &header.key(&WIDTH),
        i64::from(promise.pixels_wide),
        "/Width",
    )?;
    integer_is(
        &header.key(&HEIGHT),
        i64::from(promise.pixels_high),
        "/Height",
    )?;
    name_is(&header.key(&COLOR_SPACE), &DEVICE_RGB, "/ColorSpace")?;
    integer_is(&header.key(&BITS_PER_COMPONENT), 8, "/BitsPerComponent")?;
    name_is(&header.key(&FILTER), &FLATE, "/Filter")?;
    let pixels = image
        .stream_data()
        .map_err(|error| rejected("its image cannot be decoded", &error))?;
    match pixels {
        Some(pixels) if pixels.len() == picture.rgb.len() && pixels == picture.rgb => Ok(()),
        Some(pixels) if pixels.len() != picture.rgb.len() => {
            Err(reject("the image is not W x H x 3 bytes"))
        }
        Some(_) => Err(reject("the image's pixels are not the picture's")),
        None => Err(reject("the image cannot be decoded")),
    }
}

const FLATE: Name = Name::literal(b"/FlateDecode\0");

/// Two frame readings are the same: equal frames, bit for bit, or the same refusal rule.
fn same_reading(before: &PageFrameReading, after: &PageFrameReading) -> bool {
    match (before, after) {
        (
            PageFrameReading::Frame { width, height },
            PageFrameReading::Frame {
                width: out_width,
                height: out_height,
            },
        ) => width.to_bits() == out_width.to_bits() && height.to_bits() == out_height.to_bits(),
        (PageFrameReading::Refused { rule, .. }, PageFrameReading::Refused { rule: out, .. }) => {
            rule == out
        }
        _ => false,
    }
}

fn reject(what: &str) -> Error {
    Error::OutputRejected(format!("pdf flatten: {what}"))
}

/// A read-back that failed, as a rejection. The cause's text is not carried: it may quote the
/// document, and a rejection reaches a person.
fn rejected(what: &str, _cause: &Error) -> Error {
    reject(what)
}

/// `object` is a dictionary whose top-level keys are exactly `expected` (sorted).
fn keys_are<O: PdfObject>(object: &O, expected: &[&[u8]], what: &str) -> Result<()> {
    let mut keys = crate::pdfsyntax::dict::top_level_keys(&object.unparse())
        .map_err(|error| rejected("a dictionary cannot be read back", &error))?;
    keys.sort();
    if keys.iter().map(Vec::as_slice).eq(expected.iter().copied()) {
        Ok(())
    } else {
        Err(Error::OutputRejected(format!(
            "pdf flatten: {what} does not have exactly the keys written"
        )))
    }
}

fn dictionary_keys_are<O: PdfObject>(object: &O, expected: &[&[u8]], what: &str) -> Result<()> {
    if object.type_code() != object_type::DICTIONARY {
        return Err(Error::OutputRejected(format!(
            "pdf flatten: {what} is not a dictionary"
        )));
    }
    keys_are(object, expected, what)
}

fn name_is<O: PdfObject>(object: &O, expected: &Name, what: &str) -> Result<()> {
    if object.type_code() == object_type::NAME && object.name()? == *expected {
        Ok(())
    } else {
        Err(Error::OutputRejected(format!(
            "pdf flatten: {what} is not the name written"
        )))
    }
}

fn integer_is<O: PdfObject>(object: &O, expected: i64, what: &str) -> Result<()> {
    if object.type_code() == object_type::INTEGER && object.integer_value() == expected {
        Ok(())
    } else {
        Err(Error::OutputRejected(format!(
            "pdf flatten: {what} is not the integer written"
        )))
    }
}

/// `object` is exactly `[0 0 dw dh]`, item by item, as qpdf unparses each.
fn box_is<O: PdfObject>(object: &O, promise: &Promise, what: &str) -> Result<()> {
    let expected: [&[u8]; 4] = [
        b"0",
        b"0",
        promise.width_text.as_bytes(),
        promise.height_text.as_bytes(),
    ];
    let items: c_int = 4;
    let sound = object.type_code() == object_type::ARRAY
        && object.array_len() == items
        && (0..items)
            .zip(expected)
            .all(|(at, text)| object.array_item(at).unparse() == text);
    if sound {
        Ok(())
    } else {
        Err(Error::OutputRejected(format!(
            "pdf flatten: {what} is not [0 0 dw dh] as written"
        )))
    }
}

/// Lies the tests plant between the honest edit and the write, so that each read-back check has a
/// fake that fails it (the DoD's lying engine, ADR 0031 §4's fake list for V1 to V3).
#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "test-only lies; a slice past the end is a test bug and panics the test"
)]
pub(crate) mod lie {
    use core::cell::Cell;

    use burrow_types::Result;

    use super::{BuildsObjects, Name, PdfDocument, PdfObject, Promise};

    /// One planted lie.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Lie {
        ExtraPageKey,
        CropBoxUnequal,
        RotateNinety,
        MediaBoxUnswapped,
        SecondResource,
        ContentByteOff,
        ImageRowShort,
        ImageByteLong,
        PixelUnpainted,
        ImageExtraKey,
        WidthOff,
        OtherPageFrame,
        KeepMarkInfo,
    }

    thread_local! {
        static PLANTED: Cell<Option<Lie>> = const { Cell::new(None) };
    }

    /// Plant `lie` for the next flatten on this thread.
    pub(crate) fn plant(lie: Lie) {
        PLANTED.with(|planted| planted.set(Some(lie)));
    }

    const FOO: Name = Name::literal(b"/BurrowLie\0");

    pub(super) fn apply<D>(document: &D, page: &D::Object<'_>, promise: &Promise) -> Result<()>
    where
        D: PdfDocument,
        for<'d> D::Object<'d>: BuildsObjects,
    {
        let Some(lie) = PLANTED.with(Cell::take) else {
            return Ok(());
        };
        let rewrite_image = |bytes: &[u8]| -> Result<()> {
            let image = page
                .key(&super::RESOURCES)
                .key(&super::XOBJECT)
                .key(&super::IMAGE);
            let null = page.null_beside();
            image.replace_stream_data(bytes, &null, &null)
        };
        let picture = || -> Result<Vec<u8>> {
            let image = page
                .key(&super::RESOURCES)
                .key(&super::XOBJECT)
                .key(&super::IMAGE);
            image.stream_data()?.ok_or_else(|| {
                burrow_types::Error::Internal("the planted image cannot be read".to_owned())
            })
        };
        match lie {
            Lie::ExtraPageKey => page.replace_key(&FOO, &page.integer_beside(1)),
            Lie::CropBoxUnequal => {
                let crop = page.array_beside();
                // LARGER THAN THE MEDIA BOX, so it clips to the same frame: V1 cannot see it, and
                // only V2's own box check does.
                for value in [-1, -1, 100_000, 100_000] {
                    crop.append_item(&page.integer_beside(value))?;
                }
                page.replace_key(&super::CROP_BOX, &crop)
            }
            Lie::RotateNinety => page.replace_key(&super::ROTATE, &page.integer_beside(90)),
            Lie::MediaBoxUnswapped => {
                let (width, height) = (&promise.height_text, &promise.width_text);
                page.replace_key(&super::MEDIA_BOX, &super::frame_box(page, width, height)?)?;
                page.replace_key(&super::CROP_BOX, &super::frame_box(page, width, height)?)
            }
            Lie::SecondResource => page
                .key(&super::RESOURCES)
                .replace_key(&FOO, &page.dictionary_beside()),
            Lie::ContentByteOff => {
                let mut line = super::content_line(&promise.width_text, &promise.height_text);
                line.push(b' ');
                let null = page.null_beside();
                page.key(&super::CONTENTS)
                    .replace_stream_data(&line, &null, &null)
            }
            Lie::ImageRowShort => {
                let pixels = picture()?;
                let row = usize::try_from(promise.pixels_wide).unwrap_or(0) * 3;
                rewrite_image(&pixels[..pixels.len() - row])
            }
            Lie::ImageByteLong => {
                let mut pixels = picture()?;
                pixels.push(0);
                rewrite_image(&pixels)
            }
            Lie::PixelUnpainted => {
                let mut pixels = picture()?;
                pixels[0] ^= 0xff;
                rewrite_image(&pixels)
            }
            Lie::ImageExtraKey => page
                .key(&super::RESOURCES)
                .key(&super::XOBJECT)
                .key(&super::IMAGE)
                .stream_dict()
                .replace_key(&FOO, &page.integer_beside(1)),
            Lie::WidthOff => page
                .key(&super::RESOURCES)
                .key(&super::XOBJECT)
                .key(&super::IMAGE)
                .stream_dict()
                .replace_key(
                    &super::WIDTH,
                    &page.integer_beside(i64::from(promise.pixels_wide) + 1),
                ),
            Lie::KeepMarkInfo => {
                let root = document.trailer()?.key(&super::ROOT);
                root.replace_key(&super::MARK_INFO, &root.dictionary_beside())
            }
            Lie::OtherPageFrame => {
                let other = usize::from(promise.page == 0);
                let other = document.page(other)?;
                let media = other.array_beside();
                for value in [0, 0, 10, 10] {
                    media.append_item(&other.integer_beside(value))?;
                }
                other.replace_key(&super::MEDIA_BOX, &media)
            }
        }
    }
}

#[cfg(test)]
mod qpdf_tests;

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "tests")]
mod tests {
    use super::real_text;

    #[test]
    fn a_side_is_spelled_as_digits_that_parse_back_exactly() {
        for side in [612.0, 0.1 + 0.2, 1748.57, 14_400.0, 1e-7] {
            let text = real_text(side).unwrap();
            assert_eq!(
                text.parse::<f64>().unwrap().to_bits(),
                side.to_bits(),
                "{text}"
            );
            assert!(
                text.bytes().all(|b| b.is_ascii_digit() || b == b'.'),
                "{text}"
            );
        }
        for bad in [0.0, -3.0, f64::NAN, f64::INFINITY] {
            assert!(real_text(bad).is_err(), "{bad}");
        }
    }

    proptest::proptest! {
        /// Every accepted side round-trips through its spelling.
        #[test]
        fn every_side_round_trips(side in 0.0001_f64..14_400.0) {
            let text = real_text(side).unwrap();
            proptest::prop_assert_eq!(text.parse::<f64>().unwrap().to_bits(), side.to_bits());
        }
    }
}
