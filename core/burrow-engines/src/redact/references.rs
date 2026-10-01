//! A reference qpdf resolves to null is refused (#227).
//!
//! # What this closes
//!
//! qpdf resolves `N G R` to null when its cross-reference has no object `N` at generation `G`,
//! drops the dictionary key that held it, and raises **no warning** -- `qpdf --check` is clean and
//! a rewrite exits 0. PDFium finds object `N` by number and ignores the generation. So every entry
//! this policy reads as "absent is fine" could be absent to burrow and present to the viewer: a
//! page's `/Rotate` and `/CropBox`, a form's `/Matrix`, a font's `/Widths`, an `/ExtGState` -- each
//! measured `Ok` with the secret still drawn, on both engines, before this check. It is the third
//! way qpdf changes a document silently, after #61's lost pages and the rebuilt page tree #224
//! found, and the only one of the three the warning channel cannot report: qpdf does not think it
//! repaired anything.
//!
//! # The rule: a null the file itself declares, and nothing else
//!
//! **A reference qpdf resolves to null is refused unless the file declares that object as `null`,
//! and as nothing else**: qpdf's identity for it is the pair written; every `N … obj` header the
//! lexer reads for that number holds exactly `null`, one of them at the generation written; no
//! header failed to read; and no object stream lists the number as a member. A declared `null` is
//! read alike by both readers -- dvipdfm writes one under every catalog's `/Threads`.
//!
//! **Three rules were tried, and the record of why matters.** The owner's first refused every
//! null; measured, it refused 2 of 99 real documents against a 1% bar set before measuring, both
//! that dvipdfm `/Threads`. The second accepted a null qpdf hands back **with an identity**, on the
//! measurement that an absent pair comes back as `(0, 0)`. Both round-1 reviews broke it: qpdf
//! caches every pair it parses while it reads the cross-reference -- the trailer, a `/Prev`
//! trailer, a cross-reference stream's dictionary -- and later fills that entry with a null that
//! carries the pair, silently. One `/X 6 1 R` in the trailer, and `/Rotate 6 1 R` over a `6 0` of
//! `90` returned `Ok` with the secret drawn, on both engines. **qpdf's identity says what qpdf
//! cached, not what the file wrote**; the evidence has to be read from the file. Identity is kept
//! as a condition: it rules out a pair qpdf never cached at all.
//!
//! What it refuses: a reference to a generation the cross-reference does not have, however it got
//! into qpdf's cache; one to a number it does not have at all; one to an object in the file body
//! and missing from a valid cross-reference; one to an object stream member qpdf did not list. It
//! **over-refuses on purpose**: a reference to an object absent from both readers, which leaks
//! nothing; and a `null` member of an object stream, which has no header to read.
//!
//! **What it does not refuse by itself**, and what does: a member of an object stream qpdf cannot
//! load -- headed at a generation qpdf will not read members from, or missing `/N` or `/First`.
//! qpdf warns and stores the member as a null; this rule refuses it only if no header for the
//! number declares it otherwise, and `[engine-repaired-input]` after the write refuses it whatever
//! this rule does, because qpdf warned. ADR 0029's #227 amendment names the fixtures.
//!
//! # How
//!
//! [`crate::pdfsyntax::references`] lexes every reference from the places qpdf starts parsing:
//! after each `obj` and `trailer` keyword in the file, and inside each object stream qpdf would
//! decode. Each distinct `(number, generation)` is then asked of qpdf **with the generation as
//! written** -- never 0 in its place -- through [`PdfDocument::object`], and its type read through
//! the trapped accessor that resolves it. Null refuses.
//!
//! A reference written in any shape but plain digits is refused without asking, and so is anything
//! the lexer could not read as qpdf would: see [`crate::pdfsyntax::references::Irregular`].
//!
//! # What asking costs, stated rather than discovered
//!
//! Asking resolves every object the file refers to, where the walk resolved only what the page
//! reaches. An object qpdf repairs while resolving it now raises its warning before the write, and
//! the check after the write refuses the document as `[engine-repaired-input]`. A repaired object
//! the walk never read used to reach the output without anything having looked at it; it is now
//! refused. ADR 0029's #227 amendment records the measured count.

use core::ffi::c_int;
use std::collections::BTreeSet;

use burrow_types::{Clock, Deadline, Error, Result};

use super::graph::{PdfDocument, PdfObject};
use crate::codes::qpdf::object_type;
use crate::name::Name;
use crate::pdfsyntax::references::{self, Found, Irregular};

/// An object stream's member count.
const COUNT: Name = Name::literal(b"/N\0");

/// Where an object stream's first member begins.
const FIRST: Name = Name::literal(b"/First\0");

/// How many lookups run between two reads of the deadline.
const LOOKUPS_PER_CHECKPOINT: usize = 256;

/// The refusal for a reference qpdf resolves to null.
pub(crate) fn reference_to_nothing() -> Error {
    Error::Unsupported(
        "pdf redaction [reference-to-nothing]: the document refers to an object the PDF engine \
         reads as absent, which another reader may find and show, so what the page draws \
         cannot be known"
            .to_owned(),
    )
}

/// The refusal for references that could not be read as the PDF engine reads them.
pub(crate) fn reference_unreadable(why: Irregular) -> Error {
    let detail = match why {
        Irregular::NotPlainDigits => {
            "a reference with a sign, a leading zero or an out-of-range number"
        }
        Irregular::CommentInside => "a reference with a comment inside it",
        Irregular::StreamHeaderUnreadable => "a stream object whose header does not read",
        Irregular::ObjectStreamHeaderUnreadable => "an object stream whose header does not read",
        Irregular::BeyondCaps => "more references than redaction checks",
    };
    Error::Unsupported(format!(
        "pdf redaction [reference-unreadable]: {detail}, which readers may resolve differently, \
         so what the page draws cannot be known"
    ))
}

/// The refusal for an object stream qpdf could not decode, whose members cannot then be read.
fn object_stream_undecodable() -> Error {
    Error::Unsupported(
        "pdf redaction [reference-unreadable]: an object stream the PDF engine could not \
         decode, whose references cannot be read, so what the page draws cannot be known"
            .to_owned(),
    )
}

/// Refuse the document if any reference it writes resolves to null in `document`, or cannot be
/// read as qpdf reads it. `bytes` is the input `document` was opened from.
///
/// # Errors
///
/// `[reference-to-nothing]`, `[reference-unreadable]`, the deadline, and whatever the engine
/// reports.
pub(crate) fn refuse_references_to_nothing<D: PdfDocument>(
    document: &D,
    bytes: &[u8],
    deadline: &Deadline,
    clock: &dyn Clock,
) -> Result<()> {
    let mut checkpoint = || deadline.checkpoint(clock);
    let mut wanted = accepted(references::in_file(bytes, &mut checkpoint)?)?;
    // The numbers object streams list as members: no header in the file speaks for them.
    let mut listed: BTreeSet<c_int> = BTreeSet::new();

    // THE OBJECT STREAMS, from every stream object's header. qpdf reads an object stream's
    // members through `getObject(number, 0)` whatever generation the stream was written with
    // (`QPDF_objects.cc`, `resolveObjectsInStream`), so that is the object asked for here: the
    // one whose members qpdf would read.
    for number in core::mem::take(&mut wanted.stream_objects) {
        checkpoint()?;
        let stream = document.object(number, 0)?;
        if stream.type_code() != object_type::STREAM {
            stream.drained()?;
            continue;
        }
        let dictionary = stream.stream_dict();
        let (Some(count), Some(first)) = (
            integer_at(&dictionary, &COUNT),
            integer_at(&dictionary, &FIRST),
        ) else {
            // NOT AN OBJECT STREAM qpdf can read: it throws on a stream without both, warns, and
            // stores each member as a null. The lookups below refuse one unless a header
            // declares it null; the warning refuses the document after the write regardless.
            stream.drained()?;
            continue;
        };
        stream.drained()?;
        let Some(decoded) = stream.stream_data()? else {
            return Err(object_stream_undecodable());
        };
        // WIPED WHEN DROPPED: a member can hold a string a person wrote.
        let decoded = zeroize::Zeroizing::new(decoded);
        let members = accepted(references::in_object_stream(
            &decoded,
            count,
            first,
            &mut checkpoint,
        )?)?;
        wanted.references.extend(members.references);
        listed.extend(members.members);
        if wanted.references.len() > references::MAX_DISTINCT_REFERENCES {
            return Err(reference_unreadable(Irregular::BeyondCaps));
        }
    }

    // EVERY REFERENCE, AT THE GENERATION WRITTEN.
    for (asked, (number, generation)) in wanted.references.iter().enumerate() {
        if asked % LOOKUPS_PER_CHECKPOINT == 0 {
            checkpoint()?;
        }
        let object = document.object(*number, *generation)?;
        let resolved_to = object.type_code();
        object.drained()?;
        // NULL, AND THE TWO TYPES INTERNAL TO QPDF BELOW IT: a handle to nothing, or a placeholder
        // nothing supplied. None of them is an object a reader draws with.
        let nothing = matches!(
            resolved_to,
            object_type::NULL | object_type::UNINITIALIZED | object_type::RESERVED
        );
        // UNLESS THE FILE DECLARES THIS NULL, and qpdf holds it at this pair. See the module
        // header for why qpdf's identity alone is not a declaration.
        let declared = resolved_to == object_type::NULL
            && wanted.declares_only_null(*number, *generation)
            && !listed.contains(number)
            && object.object()? == (*number, *generation);
        if nothing && !declared {
            return Err(reference_to_nothing());
        }
    }
    Ok(())
}

/// The scan's answer, or the refusal it calls for.
fn accepted(found: Found) -> Result<Found> {
    match found.irregular {
        Some(why) => Err(reference_unreadable(why)),
        None => Ok(found),
    }
}

/// The integer at `key` in `dictionary`, asking the type first, or `None`.
fn integer_at<O: PdfObject>(dictionary: &O, key: &Name) -> Option<i64> {
    let value = dictionary.key(key);
    (value.type_code() == object_type::INTEGER).then(|| value.integer_value())
}
