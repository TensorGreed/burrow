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
//! the lexer could not read as qpdf would: see [`crate::pdfsyntax::references::Irregular`]. So is
//! a name PDFium and qpdf are not measured to read alike, anywhere the scan lexes, under its own
//! refusal (owner, 2026-10-14; see [`crate::pdfsyntax::references::name_reads_alike`]): the scan
//! already lexes every value qpdf parses, so it is where object syntax is read as written.
//!
//! # What asking costs, stated rather than discovered
//!
//! Asking resolves every object the file refers to, where the walk resolved only what the page
//! reaches. An object qpdf repairs while resolving it now raises its warning before the write, and
//! the check after the write refuses the document as `[engine-repaired-input]`. A repaired object
//! the walk never read used to reach the output without anything having looked at it; it is now
//! refused. ADR 0029's #227 amendment records the measured count.

use core::ffi::c_int;
use std::collections::{BTreeMap, BTreeSet};

use burrow_types::{Clock, Deadline, Error, Result};

use super::graph::{PdfDocument, PdfObject};
use crate::codes::qpdf::object_type;
use crate::name::Name;
use crate::pdfsyntax::references::{self, EmptyKey, Found, Holder, Irregular, Referrers};

/// An object stream's member count.
const COUNT: Name = Name::literal(b"/N\0");

/// Where an object stream's first member begins.
const FIRST: Name = Name::literal(b"/First\0");

/// The trailer's catalog.
const ROOT: Name = Name::literal(b"/Root\0");
const DESTS: Name = Name::literal(b"/Dests\0");

/// How many lookups run between two reads of the deadline.
const LOOKUPS_PER_CHECKPOINT: usize = 256;

/// The refusals the check raises, named by the operation that runs it.
///
/// **Carried by the caller, as the lexer's `InlineImages` carries redaction's** (#228): split runs
/// this check at its open too (owner, 2026-10-13), and split is in the base web module, which must
/// not carry redaction's refusal text (`tools/check-redaction-not-in-base.sh`). Each operation
/// names its own functions, and the other's are unreferenced wherever it is absent.
#[derive(Clone, Copy)]
pub(crate) struct ReferenceRefusals {
    /// A reference qpdf resolves to null.
    pub(crate) nothing: fn() -> Error,
    /// References that could not be read as qpdf reads them, and why.
    pub(crate) unreadable: fn(Irregular) -> Error,
    /// An object stream qpdf could not decode.
    pub(crate) undecodable: fn() -> Error,
    /// A name PDFium and qpdf are not measured to read alike.
    pub(crate) name: fn() -> Error,
    /// A byte in object syntax refused fail closed (`Irregular::SyntaxReadDifferently`).
    pub(crate) syntax: fn() -> Error,
    /// A dictionary value qpdf would drop on write -- the literal `null`, or a reference to an
    /// object the file declares `null` -- for the operations that refuse one (split); `None`
    /// where the operation does not (yet) judge it.
    pub(crate) null_value: Option<fn() -> Error>,
    /// Whether the null-value refusal waits for the caller ([`Referenced::drops_a_null`]) rather
    /// than refusing in this pass: redaction's, so that the refusals its evasion fixtures pin
    /// still name themselves first.
    pub(crate) defer_null_value: bool,
    /// Whether an empty name written as a key of the catalog's `/Dests` dictionary passes this
    /// check: split's open (owner, 2026-10-15). Admitted only where the catalog's `/Dests` entry is
    /// that dictionary's **sole referrer** -- no other dictionary, array, name tree, object-stream
    /// member or trailer names it -- or where the `/Dests` dictionary is written directly in the
    /// catalog; and in both cases only where nothing but a trailer names the catalog itself, so
    /// nothing a part receives can reach either. An xref stream's `/Root` counts as an object's
    /// reference, so such a file refuses: an over-refusal, recorded. An operation that sets this
    /// also holds every output to the same pass with it unset ([`Referenced::admitted_empty_keys`]
    /// says when), as a second line of defence. Every other empty key refuses as [`Self::name`],
    /// and every empty key refuses where this is `false`.
    pub(crate) admit_dests_empty_key: bool,
}

/// Redaction's refusals: `[reference-to-nothing]` and `[reference-unreadable]`.
pub(crate) const REDACTION: ReferenceRefusals = ReferenceRefusals {
    nothing: reference_to_nothing,
    unreadable: reference_unreadable,
    undecodable: object_stream_undecodable,
    name: name_read_differently,
    syntax: syntax_read_differently,
    null_value: Some(null_value),
    defer_null_value: true,
    // REDACTION KEEPS THE CATALOG, so an empty key there would reach the document it hands back,
    // and an empty key was not measured as alike (owner, 2026-10-15).
    admit_dests_empty_key: false,
};

/// What an irregular reference was, for any operation's refusal of it. `operation` names who
/// checks, for the one case that says so: more references than it checks.
pub(crate) const fn irregular_detail(why: Irregular, operation: Operation) -> &'static str {
    match why {
        Irregular::NotPlainDigits => {
            "a reference with a sign, a leading zero or an out-of-range number"
        }
        Irregular::CommentInside => "a reference with a comment inside it",
        Irregular::StreamHeaderUnreadable => "a stream object whose header does not read",
        Irregular::ObjectStreamHeaderUnreadable => "an object stream whose header does not read",
        Irregular::BeyondCaps => match operation {
            Operation::Redaction => "more references than redaction checks",
            Operation::Split => "more references than split checks",
        },
        // UNREACHABLE THROUGH `accepted`, which refuses this one with `ReferenceRefusals::name`; the
        // arm exists because the match is exhaustive.
        Irregular::NameReadDifferently => "a name readers may read differently",
        // UNREACHABLE THROUGH `accepted` too, which refuses with `ReferenceRefusals::syntax`.
        Irregular::SyntaxReadDifferently => "a byte in object syntax readers may read differently",
    }
}

/// Which operation an [`irregular_detail`] is for.
#[derive(Clone, Copy)]
pub(crate) enum Operation {
    /// Redaction, whose text is unchanged since #227.
    Redaction,
    /// Split's open (owner, 2026-10-13).
    Split,
}

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
    let detail = irregular_detail(why, Operation::Redaction);
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

/// The refusal for a name PDFium and qpdf are not measured to read alike (owner, 2026-10-14).
fn name_read_differently() -> Error {
    Error::Unsupported(
        "pdf redaction [name-read-differently]: the document writes a name that readers may \
         read differently, so what the page draws cannot be known"
            .to_owned(),
    )
}

/// The refusal for a byte in object syntax refused fail closed (owner, 2026-10-14).
fn syntax_read_differently() -> Error {
    Error::Unsupported(
        "pdf redaction [syntax-read-differently]: the document writes object syntax that readers \
         may read differently, so what the page draws cannot be known"
            .to_owned(),
    )
}

/// The refusal for a dictionary value qpdf drops when it writes (owner, 2026-10-14).
pub(crate) fn null_value() -> Error {
    Error::Unsupported(
        "pdf redaction [null-value]: the document writes a value the PDF engine drops when it \
         writes, so what the page draws cannot be known"
            .to_owned(),
    )
}

/// What the reference pass read, kept for the walk that asks what names a removed annotation
/// (#239): every reference in the file, its object streams and the trailer qpdf holds, at the
/// generation written, and those of that trailer apart.
#[derive(Debug, Default, Clone)]
pub(crate) struct Referenced {
    pub(crate) objects: BTreeSet<(c_int, c_int)>,
    pub(crate) from_trailer: BTreeSet<(c_int, c_int)>,
    /// Whether the document writes a dictionary value qpdf drops, where the refusal was deferred.
    pub(crate) drops_a_null: bool,
    /// Whether an empty key passed because its operation admits the catalog's `/Dests`: the
    /// caller must then re-run this pass, with the admission off, over everything it writes.
    pub(crate) admitted_empty_keys: bool,
}

/// Refuse the document if any reference it writes resolves to null in `document`, or cannot be
/// read as qpdf reads it. `bytes` is the input `document` was opened from. Returns what it read.
///
/// # Errors
///
/// `[reference-to-nothing]`, `[reference-unreadable]`, `[name-read-differently]`,
/// `[syntax-read-differently]`, the deadline, and whatever the engine reports.
pub(crate) fn refuse_references_to_nothing<D: PdfDocument>(
    document: &D,
    bytes: &[u8],
    deadline: &Deadline,
    clock: &dyn Clock,
) -> Result<Referenced> {
    refuse_references_to_nothing_as(document, bytes, deadline, clock, &REDACTION)
}

/// [`refuse_references_to_nothing`], refusing with `refusals`: the check itself, for any operation.
///
/// # Errors
///
/// `refusals`' errors, the deadline, and whatever the engine reports.
pub(crate) fn refuse_references_to_nothing_as<D: PdfDocument>(
    document: &D,
    bytes: &[u8],
    deadline: &Deadline,
    clock: &dyn Clock,
    refusals: &ReferenceRefusals,
) -> Result<Referenced> {
    let mut checkpoint = || deadline.checkpoint(clock);
    let mut wanted = accepted(references::in_file(bytes, &mut checkpoint)?, refusals)?;
    // THE NULL VALUES, from the file and from every object stream, kept apart from `wanted`'s
    // references so the object streams' merge below does not need to know about them.
    let mut null_holders = core::mem::take(&mut wanted.null_values);
    let mut dictionary_references = core::mem::take(&mut wanted.dictionary_references);
    let mut empty_keys = core::mem::take(&mut wanted.empty_keys);
    let mut referrers = core::mem::take(&mut wanted.referrers);
    // The numbers object streams list as members: no header in the file speaks for them. Capped
    // per stream at `MAX_DECLARATIONS`, not in total: memory here is detected, not bounded
    // (ADR 0007), and every stream's decoded bytes are already held while it is read.
    let mut listed: BTreeSet<c_int> = BTreeSet::new();
    // HOW MANY OBJECT STREAMS list each number: a number listed twice, or listed and also headed in
    // the file body, is more than one revision of the object.
    let mut listings: Vec<c_int> = Vec::new();

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
            return Err((refusals.undecodable)());
        };
        // WIPED WHEN DROPPED: a member can hold a string a person wrote.
        let decoded = zeroize::Zeroizing::new(decoded);
        let members = accepted(
            references::in_object_stream(&decoded, count, first, &mut checkpoint)?,
            refusals,
        )?;
        wanted.references.extend(members.references);
        // ENTRIES, not streams: a number listed twice in one stream is two copies.
        listings.extend_from_slice(&members.member_list);
        listed.extend(members.members);
        null_holders.extend(members.null_values);
        dictionary_references.extend(members.dictionary_references);
        empty_keys.extend(members.empty_keys);
        for (target, counted) in &members.referrers {
            referrers.entry(*target).or_default().absorb(counted);
        }
        if wanted.references.len() > references::MAX_DISTINCT_REFERENCES {
            return Err((refusals.unreadable)(Irregular::BeyondCaps));
        }
    }

    // THE TRAILER QPDF HOLDS, and every reference in it (#239). The scan above finds trailers in
    // the bytes, and qpdf's grammar has corners it does not share: a `trailer` keyword written
    // straight after a digit (`11 0trailer`) is one qpdf reads without a word and the scan never
    // saw -- a removed annotation that trailer named was written out after an `Ok`, measured.
    // Asked of qpdf, it is the trailer qpdf will write, whatever section it came from. Its
    // references join the set so that #239's walk reads the objects only it names. For the null
    // check below they are **defence in depth, unwitnessed**: qpdf drops a trailer key whose
    // reference is dangling before this sees it (measured by #239's fourth code review), so none of
    // them can resolve to nothing here.
    let trailer = document.trailer()?;
    let text = zeroize::Zeroizing::new(trailer.unparse());
    trailer.drained()?;
    let from_trailer =
        accepted(references::in_value(&text, &mut checkpoint)?, refusals)?.references;
    wanted.references.extend(from_trailer.iter().copied());
    if wanted.references.len() > references::MAX_DISTINCT_REFERENCES {
        return Err((refusals.unreadable)(Irregular::BeyondCaps));
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
        // `resolved_to == NULL` is defence in depth, unwitnessed: a header declaring `null` whose
        // object qpdf hands back as uninitialized or reserved is a state qpdf warns about, which
        // refuses after the write; no test reaches it otherwise.
        let declared = resolved_to == object_type::NULL
            && wanted.declares_only_null(*number, *generation)
            && !listed.contains(number)
            && object.object()? == (*number, *generation);
        if nothing && !declared {
            return Err((refusals.nothing)());
        }
    }
    // EVERY EMPTY KEY REFUSES, unless the operation admits the catalog's `/Dests` dictionary,
    // that is the object holding it, and nothing but the catalog's `/Dests` entry refers to it.
    let admitted_empty_keys = !empty_keys.is_empty();
    if admitted_empty_keys {
        let slots = if refusals.admit_dests_empty_key {
            catalog_dests(document, &wanted, &listings)?
        } else {
            DestsSlots::default()
        };
        // THE CATALOG ITSELF must reach no part either: only a trailer may name it. A form's
        // resources, a graphics state or an annotation naming it would carry its `/Dests`, direct
        // or indirect, into whatever part copies them (round 2's review).
        let catalog_alone = slots
            .catalog
            .is_some_and(|catalog| named_by_trailers_only(catalog, &referrers));
        let sole = slots
            .indirect
            .is_some_and(|dests| sole_referrer(dests, slots.catalog, &referrers, &from_trailer));
        let admitted = |key: &EmptyKey| {
            catalog_alone
                && if key.in_direct_dests {
                    slots.catalog == Some(key.object)
                } else {
                    slots.indirect == Some(key.object) && sole
                }
        };
        if !empty_keys.iter().all(admitted) {
            return Err((refusals.name)());
        }
    }
    let mut drops_a_null = false;
    if let Some(refuse) = refusals.null_value
        && null_value_found(
            document,
            &wanted,
            &listed,
            &listings,
            &null_holders,
            &dictionary_references,
        )?
    {
        if !refusals.defer_null_value {
            return Err(refuse());
        }
        drops_a_null = true;
    }
    Ok(Referenced {
        objects: wanted.references,
        from_trailer,
        drops_a_null,
        admitted_empty_keys,
    })
}

/// Whether the document writes a dictionary value qpdf drops when it writes: the literal `null`, or a reference to an
/// object the file declares `null` (the /OC-null slice; owner, 2026-10-14). A reader that treats the
/// key's presence as meaning something reads the source and the part differently.
///
/// **One exemption:** a direct key of the catalog dictionary itself. For split, because no part
/// receives the source catalog's keys. For redaction, which keeps the catalog, the justification is
/// narrower and is a residual (owner, 2026-10-14): read in pdf.js 5.7.284's source, a null catalog
/// key differs from an absent one only for `/StructTreeRoot` (a metadata flag), `/PageLabels`
/// through a reference to null (the labels) and `/Dests` (caching) -- none changes what is drawn;
/// no other reader was measured. The catalog is the `/Root` qpdf resolves from the
/// trailer, as the stopgap's check reads it, and only when its number is written exactly once in
/// the file -- one header at that generation, or one object stream's member -- so an earlier
/// revision's catalog, a second `/Type /Catalog` object, and anything nested inside the catalog are
/// judged like any other dictionary. The census: 0 of 99 real documents, where refusing the
/// catalog's keys too cost 3.
fn null_value_found<D: PdfDocument>(
    document: &D,
    wanted: &Found,
    listed: &BTreeSet<c_int>,
    listings: &[c_int],
    null_holders: &[Holder],
    dictionary_references: &[((i32, i32), Holder)],
) -> Result<bool> {
    let trailer = document.trailer()?;
    let root = trailer.key(&ROOT);
    let catalog = if root.type_code() == object_type::DICTIONARY {
        written_once(wanted, listings, root.object()?)
    } else {
        None
    };
    root.drained()?;
    trailer.drained()?;
    let exempt = |holder: &Holder| holder.direct && catalog.is_some() && holder.object == catalog;
    let literal = null_holders.iter().any(|holder| !exempt(holder));
    let declared = dictionary_references
        .iter()
        .any(|((number, generation), holder)| {
            !exempt(holder)
                && wanted.declares_only_null(*number, *generation)
                && !listed.contains(number)
        });
    Ok(literal || declared)
}

/// `identity`'s number, when that object is written exactly once in the file: one header, at the
/// generation qpdf resolved, and in no object stream; or one object stream's member and no header.
/// `None` for anything else -- written twice (an earlier revision, or a header and a member), at
/// another generation, or numbered 0 -- so a check keyed on it applies to no object. Shared by the
/// null-value exemption's catalog and by the catalog's `/Dests`.
fn written_once(wanted: &Found, listings: &[c_int], identity: (c_int, c_int)) -> Option<c_int> {
    let (number, generation) = identity;
    let headed = wanted.declarations.get(&number);
    let headers = headed.map_or(0, |declared| declared.headers);
    let listed_times = listings.iter().filter(|listed| **listed == number).count();
    let once = match (headers, listed_times) {
        (1, 0) => headed.is_some_and(|declared| declared.generations.contains(&generation)),
        (0, 1) => true,
        _ => false,
    };
    (number > 0 && once).then_some(number)
}

/// Where an empty key may stand, as [`catalog_dests`] found the catalog.
#[derive(Default)]
struct DestsSlots {
    /// The catalog's number, written once: an empty key of a `/Dests` dictionary written directly
    /// in it is admitted, since a direct object has no other referrer.
    catalog: Option<c_int>,
    /// The catalog's indirect `/Dests` dictionary, written once and not the catalog itself: an
    /// empty key of its own dictionary is admitted when the catalog's entry is its sole referrer.
    indirect: Option<c_int>,
}

/// The catalog's `/Dests` slots: the `/Root` qpdf resolves from the trailer, written once, and its
/// `/Dests` value. Both `None` for anything else -- no catalog, either written twice or never, or a
/// `/Dests` naming the catalog itself -- so no empty key is admitted then.
fn catalog_dests<D: PdfDocument>(
    document: &D,
    wanted: &Found,
    listings: &[c_int],
) -> Result<DestsSlots> {
    let trailer = document.trailer()?;
    let root = trailer.key(&ROOT);
    let mut slots = DestsSlots::default();
    if root.type_code() == object_type::DICTIONARY
        && let Some(catalog) = written_once(wanted, listings, root.object()?)
    {
        let dests = root.key(&DESTS);
        if dests.type_code() == object_type::DICTIONARY {
            let identity = dests.object()?;
            if identity == (0, 0) {
                slots.catalog = Some(catalog);
            } else if let Some(number) = written_once(wanted, listings, identity)
                // DEFENCE IN DEPTH, unwitnessed: a catalog that is its own `/Dests` is also named
                // by the trailer's `/Root`, so `sole_referrer` refuses it first, and no fixture can
                // tell this apart (the mutation removing it survives, measured).
                && number != catalog
            {
                slots.catalog = Some(catalog);
                slots.indirect = Some(number);
            }
        }
        dests.drained()?;
    }
    root.drained()?;
    trailer.drained()?;
    Ok(slots)
}

/// Whether the catalog's `/Dests` entry is the only reference to `dests` anywhere the scan read:
/// every object's dictionaries and arrays, every object stream's members, every trailer in the
/// file, and the trailer qpdf holds. One occurrence, the `/Dests` value of the catalog's own
/// dictionary, and no other -- so a form's or a page's resources, a name tree, or any other holder
/// that names it refuses (owner, 2026-10-15). A reference whose number did not read refuses the
/// document earlier, so none goes uncounted here.
fn sole_referrer(
    dests: c_int,
    catalog: Option<c_int>,
    referrers: &BTreeMap<c_int, Referrers>,
    from_trailer: &BTreeSet<(c_int, c_int)>,
) -> bool {
    #[cfg(test)]
    if crate::redact::hooks::sole_referrer_skipped() {
        return true;
    }
    let Some(counted) = referrers.get(&dests) else {
        return false;
    };
    counted.sites == 1
        // DEFENCE IN DEPTH, unwitnessed (each clause's removal survives, measured): with one site
        // in all, that it is the catalog's `/Dests` entry is what `catalog_dests` already
        // established, and a reference in qpdf's trailer is one in the file's, already counted.
        && counted.dests_entries == 1
        && counted.dests_entry_holder.is_some()
        && counted.dests_entry_holder == catalog
        && !from_trailer.iter().any(|(number, _)| *number == dests)
}

/// Whether every reference to `catalog` the scan read is written in a trailer: no object -- a
/// form's resources, a graphics state, an annotation, anything -- names the catalog, so no part
/// that copies an object can receive it.
///
/// **An xref stream's `/Root` counts as an object's reference**, since the cross-reference stream
/// is an object, so a file whose `/Root` lives only in an xref-stream dictionary refuses even when
/// it is otherwise admissible: an over-refusal, recorded (final review of #136's Chrome narrowing).
fn named_by_trailers_only(catalog: c_int, referrers: &BTreeMap<c_int, Referrers>) -> bool {
    referrers
        .get(&catalog)
        .is_none_or(|counted| counted.held == 0)
}

/// The scan's answer, or the refusal it calls for.
fn accepted(found: Found, refusals: &ReferenceRefusals) -> Result<Found> {
    match found.irregular {
        Some(Irregular::NameReadDifferently) => Err((refusals.name)()),
        Some(Irregular::SyntaxReadDifferently) => Err((refusals.syntax)()),
        Some(why) => Err((refusals.unreadable)(why)),
        None => Ok(found),
    }
}

/// The integer at `key` in `dictionary`, asking the type first, or `None`.
fn integer_at<O: PdfObject>(dictionary: &O, key: &Name) -> Option<i64> {
    let value = dictionary.key(key);
    (value.type_code() == object_type::INTEGER).then(|| value.integer_value())
}

#[cfg(test)]
mod tests {
    use super::written_once;
    use crate::pdfsyntax::references::{Declared, Found};

    fn headed(number: i32, generations: &[i32]) -> Found {
        let mut found = Found::default();
        found.declarations.insert(
            number,
            Declared {
                headers: generations.len(),
                generations: generations.iter().copied().collect(),
                ..Declared::default()
            },
        );
        found
    }

    /// WRITTEN ONCE, branch by branch: one header at the generation asked; one object-stream member
    /// and no header; and the refusals -- a header at another generation, two headers, a header and
    /// a member, two members, and the number 0.
    #[test]
    fn written_once_holds_only_for_one_copy_at_the_generation_asked() {
        assert_eq!(written_once(&headed(6, &[0]), &[], (6, 0)), Some(6));
        assert_eq!(
            written_once(&headed(6, &[0]), &[], (6, 1)),
            None,
            "a header at another generation"
        );
        assert_eq!(
            written_once(&headed(6, &[0, 0]), &[], (6, 0)),
            None,
            "two headers"
        );
        assert_eq!(written_once(&Found::default(), &[6], (6, 0)), Some(6));
        assert_eq!(
            written_once(&headed(6, &[0]), &[6], (6, 0)),
            None,
            "a header and a member"
        );
        assert_eq!(
            written_once(&Found::default(), &[6, 6], (6, 0)),
            None,
            "two members"
        );
        assert_eq!(
            written_once(&headed(0, &[0]), &[], (0, 0)),
            None,
            "number 0"
        );
    }
}
