//! Assert that an operation's output contains no object belonging to content it excluded.
//!
//! **A shared harness, not a `split` test.** Any operation that emits a document derived from
//! another can use it, and the ones that are not subsetting operations use it to assert the
//! opposite: `rotate`, `reorder` and `compress` keep every page, so *every* marked object
//! should survive, and this harness says so with the same call.
//!
//! # Why this rather than a list of canaries
//!
//! A canary list can only ever confirm the enumeration it was given, and its failure mode is
//! that it passes. `split`'s first leak test planted twenty canaries across four features --
//! outline, field, annotation, attachment -- while the thing that decided whether data crossed
//! was object *sharing*, and the fixture gave every page its own objects. Twenty measurements
//! of a case that was never at risk (ADR 0019 §2a).
//!
//! This asserts a **property**: every object surviving in the output belongs to a page the
//! output contains. It fails for categories nobody named, which is the point.
//!
//! # Ownership is declared by the fixture, not derived from the graph
//!
//! `tools/make-marked-document.py` writes a manifest saying which pages each object belongs
//! to. Reachability cannot answer that question, and gets it wrong in both directions: an
//! inherited `/Resources` dictionary is reachable from every page through `/Parent` and belongs
//! to whichever page draws it; an `/Annots` array shared by two pages makes the second page's
//! annotation reachable from the first, and it still belongs to the second.
//!
//! # What it cannot see, stated rather than left to be discovered
//!
//! - **A leak inside a shared object.** A form field owned by pages 1 and 4 legitimately
//!   survives into an output containing page 1, carrying the `/V` somebody typed on page 4.
//!   Sub-object leaks need the named-channel regression cases that sit on top of this.
//! - **Anything only present in transformed form.** `qpdf --qdf` does not decode `/DCTDecode`,
//!   `/JPXDecode` or `/JBIG2Decode`, so a font subset still carrying glyphs for removed
//!   characters, or an image's pixels, is outside its reach. For redaction that transformed
//!   form *is* the leak, so M2 must not inherit this as though it were sufficient.
//! - **An object with nowhere to carry a marker.** Arrays are declared `unmarkable` in the
//!   manifest rather than quietly omitted, and this harness reports the count so a manifest
//!   that stopped marking things is visible rather than reading as a clean run.
//!
//!   **Page dictionaries joined them when #54 closed, and every consumer of this fixture should
//!   know why.** `split`'s pruning removes every page key outside the specified set — the rule
//!   that closes `/B`, `/AA`, `/Thumb` and everything nobody enumerated — and a `/BM` marker is
//!   exactly such a key. Marking a page would measure the rule rather than the page, and a
//!   correct split would report five lost pages. `rotate`, `reorder` and `compress` used those
//!   markers as evidence the page object survived; what witnesses it now is the page's content
//!   stream, which is marked, owned by the same page, and required by `assert_nothing_lost` for
//!   all three. That is a real substitution rather than a gap, and it is stated here because the
//!   three operations that rely on it will not read `split`'s ADR.

#![allow(dead_code, clippy::expect_used, clippy::panic, clippy::print_stdout)]

/// Reading a document back without a PDF parser — decompression, arrays, the page tree.
///
/// Declared here rather than beside this module so a test binary that wants both gets one
/// copy: every consumer of this harness reaches it as `object_closure::pdf_reading`.
#[path = "pdf_reading.rs"]
pub mod pdf_reading;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One object's entry in the manifest.
#[derive(Debug, Clone)]
pub struct MarkedObject {
    /// The literal that appears in the document if this object survives.
    pub marker: String,
    /// One-based page numbers this object belongs to. Empty means document scaffolding.
    pub pages: Vec<u64>,
    /// `content` — what a page is made of, which any operation keeping the page must keep.
    /// `navigation` — document-level furniture pointing at pages, which a subsetting operation
    /// may drop (ADR 0019 §1 does, and §4 has the wording `/split-pdf` will say it in) and a
    /// page-preserving one may not.
    ///
    /// **`navigation` covers more than outlines**, and it grew when #54 closed. A form field
    /// tree and an article thread are document-level furniture by the same definition: the
    /// field is reached from the catalog's `/AcroForm` and describes the whole document, the
    /// thread from `/Threads`, and `split` drops both — the first by cutting every widget's
    /// `/Parent`, the second by leaving `/B` off the page-key allowlist (ADR 0019 §2b). The
    /// widgets themselves stay `content`: they are annotations on their own pages and a split
    /// that lost them would have lost something a page needs.
    pub kind: String,
}

/// A marked document and what its objects belong to.
#[derive(Debug, Clone)]
pub struct Marked {
    pub bytes: Vec<u8>,
    pub pages: u64,
    pub objects: BTreeMap<u64, MarkedObject>,
    /// Objects the fixture could not mark, with the reason. Reported, never silently dropped.
    pub unmarkable: usize,
    /// Objects the manifest names by role, so a test need not hardcode a number.
    ///
    /// The numbers shift whenever the generator gains an object, and a test pinned to a literal
    /// would then assert something about whatever moved into its place — silently, and in the
    /// direction of passing.
    pub roles: BTreeMap<String, u64>,
}

/// Generate the marked document, once per process.
///
/// Once, because cargo runs a file's tests in parallel threads of one process and a shared
/// path written while being read is a half-written file -- which `split_no_leak.rs` met and
/// reported as a damaged fixture.
pub fn marked_document() -> Marked {
    static ONCE: std::sync::OnceLock<Marked> = std::sync::OnceLock::new();
    ONCE.get_or_init(build_marked).clone()
}

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is whichever crate is running the test, so climb until the
    // generator is found rather than assuming a depth.
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for _ in 0..4 {
        if dir.join("tools/make-marked-document.py").exists() {
            return dir;
        }
        dir = dir.parent().map(Path::to_path_buf).unwrap_or(dir);
    }
    panic!("could not find tools/make-marked-document.py from the crate root");
}

fn build_marked() -> Marked {
    let dir = std::env::temp_dir().join(format!("burrow-marked-{}", std::process::id()));
    let status = Command::new("python3")
        .arg(repo_root().join("tools/make-marked-document.py"))
        .arg(&dir)
        .stdout(Stdio::null())
        .status()
        .expect("python3 is needed to generate the marked document");
    assert!(status.success(), "the marked-document generator failed");

    let bytes = std::fs::read(dir.join("marked.pdf")).expect("marked.pdf");
    let manifest = std::fs::read_to_string(dir.join("marked.json")).expect("marked.json");

    // A hand-rolled reader for a file this crate generates, rather than a serde dependency in
    // test support. The shape is fixed by the generator and asserted below.
    let pages = field_number(&manifest, "\"pages\":").expect("manifest has a page count");
    let mut objects = BTreeMap::new();
    for chunk in manifest.split("\"marker\":").skip(1) {
        let marker = between(chunk, '"', '"').expect("a marker string");
        let pages_list = chunk
            .split_once("\"pages\":")
            .and_then(|(_, rest)| between(rest, '[', ']'))
            .expect("a pages list");
        // EVERY TOKEN MUST PARSE. `filter_map(...ok())` silently discarded anything it did
        // not understand, and an empty `owners` is indistinguishable from declared
        // scaffolding -- which `trespassers` always allows. So a change to how the generator
        // writes `pages` would have turned every object into scaffolding and made
        // `assert_closed` pass vacuously. Found by security review; failing open is the one
        // direction a leak check may never fail.
        let tokens: Vec<&str> = pages_list
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect();
        let owners: Vec<u64> = tokens
            .iter()
            .filter_map(|t| t.parse::<u64>().ok())
            .collect();
        assert_eq!(
            owners.len(),
            tokens.len(),
            "marker {marker}: the manifest's page list {pages_list:?} is not all numbers, so \
             this object would have been treated as scaffolding and allowed anywhere"
        );
        let number = marker
            .rsplit('-')
            .next()
            .and_then(|n| n.parse::<u64>().ok())
            .expect("a marker ends in its object number");
        let kind = chunk
            .split_once("\"kind\":")
            .and_then(|(_, rest)| between(rest, '"', '"'))
            .unwrap_or("content")
            .to_owned();
        objects.insert(
            number,
            MarkedObject {
                marker: marker.to_owned(),
                pages: owners,
                kind,
            },
        );
    }
    // `"object":` appears in both `unmarkable` and `roles`, so counting it over the whole file
    // counts roles as unmarkable objects. The `unmarkable` array is what this number is about.
    let unmarkable = between(
        manifest
            .split_once("\"unmarkable\":")
            .map_or("", |(_, rest)| rest),
        '[',
        ']',
    )
    .map_or(0, |list| list.matches("\"object\":").count());

    // The named roles: `"<name>": N`, bare numbers rather than objects with fields of their
    // own. They deliberately carry no `"marker"`, because the object reader above finds objects
    // by splitting on `"marker":` -- a role that had one was read as an extra object and every
    // test in every consumer failed at once.
    let mut roles = BTreeMap::new();
    if let Some((_, block)) = manifest.split_once("\"roles\":") {
        for line in block.lines().skip(1) {
            let line = line.trim().trim_end_matches(',');
            if line.starts_with('}') {
                break;
            }
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let Some(number) = value.trim().parse::<u64>().ok() else {
                continue;
            };
            roles.insert(name.trim().trim_matches('"').to_owned(), number);
        }
    }

    // EVERY ROLE NAMES AN OBJECT THAT IS REALLY THERE. A role pointing at nothing is a test
    // that asserts about an absent object, which passes whatever the operation did.
    assert!(
        !roles.is_empty(),
        "the manifest declares no roles, so every test that asks for one by name would be \
         asserting about nothing"
    );

    Marked {
        bytes,
        pages,
        objects,
        unmarkable,
        roles,
    }
}

fn between(text: &str, open: char, close: char) -> Option<&str> {
    let start = text.find(open)? + open.len_utf8();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

fn field_number(text: &str, key: &str) -> Option<u64> {
    let after = text.split_once(key)?.1;
    after
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// `qpdf --qdf` the bytes, so a marker inside a compressed stream is findable.
///
/// **Re-exported, not implemented here.** It used to be a copy, and `reorder` then wrote a
/// third one — which is how a lesson already recorded in two files cost six failing tests
/// again. `pdf_reading` is the one home for reading output back, and the reasoning lives
/// there with it.
pub use pdf_reading::expanded;

/// Which marked objects survived into `output`.
///
/// An ASCII window search, unlike `split_no_leak.rs`'s `contains`, which also matches UTF-16BE.
/// That is deliberate rather than an oversight in one of two sibling harnesses: these markers
/// are our own literal strings in `/BM`, and qpdf round-trips a literal string unchanged. If it
/// ever hex-encoded them, every marker would stop matching at once and `assert_closed`'s
/// no-survivors control fires loudly rather than reporting a clean run.
pub fn survivors(marked: &Marked, output: &[u8]) -> Vec<u64> {
    let text = expanded(output);
    marked
        .objects
        .iter()
        .filter(|(_, object)| {
            text.windows(object.marker.len())
                .any(|w| w == object.marker.as_bytes())
        })
        .map(|(number, _)| *number)
        .collect()
}

/// What an output containing `included` pages carried that it should not have.
///
/// Returns `(object number, marker, owning pages)` for every survivor belonging only to pages
/// the output does not contain. Scaffolding -- objects owned by no page -- is always allowed.
pub fn trespassers(
    marked: &Marked,
    output: &[u8],
    included: &[u64],
) -> Vec<(u64, String, Vec<u64>)> {
    survivors(marked, output)
        .into_iter()
        .filter_map(|number| {
            let object = marked.objects.get(&number)?;
            if object.pages.is_empty() || object.pages.iter().any(|p| included.contains(p)) {
                return None;
            }
            Some((number, object.marker.clone(), object.pages.clone()))
        })
        .collect()
}

/// Assert that everything which had to survive did.
///
/// **The inverse assertion, and it is a genuinely different one.** `assert_closed` asks whether
/// anything trespassed; with every page included, that question is *mathematically vacuous* —
/// every owned object belongs to an included page and scaffolding is always allowed, so the
/// trespasser list is empty whatever the operation did. Measured: `assert_closed` accepted a
/// page-1-only output while being told all five pages were included, which is a "must lose
/// nothing" operation losing four pages in five and passing.
///
/// So `rotate`, `reorder` and `compress` — which keep every page and must lose nothing — call
/// **this**, not `assert_closed` with every page. It names the objects that must be there, so
/// it fails when one is missing.
///
/// Why a named set rather than "all of them": a one-way `split` legitimately loses the
/// catalog-level furniture ADR 0019 §1 drops, so `survivors == objects` is false for a correct
/// implementation. The caller states what its operation is required to keep, and a `must` list
/// that went empty would make this vacuous in the other direction — so an empty one is refused.
pub fn assert_nothing_lost(marked: &Marked, output: &[u8], must: &[u64], what: &str) {
    assert!(
        !must.is_empty(),
        "{what}: assert_nothing_lost was given nothing that must survive, which asserts nothing"
    );
    let survived = survivors(marked, output);
    let lost: Vec<(u64, String)> = must
        .iter()
        .filter(|n| !survived.contains(n))
        .filter_map(|n| marked.objects.get(n).map(|o| (*n, o.marker.clone())))
        .collect();
    println!(
        "  {what}: {} of {} required objects survived ({} marked in total)",
        must.len() - lost.len(),
        must.len(),
        marked.objects.len()
    );
    assert!(
        lost.is_empty(),
        "{what}: {} object(s) that had to survive are missing from the output: {lost:?}",
        lost.len()
    );
}

/// Every object of `kind` that belongs to at least one of `pages`.
///
/// What a caller passes to [`assert_nothing_lost`]. The kind is the caller's obligation stated
/// explicitly, and there are three:
///
/// | kind | what it is | who may lose it |
/// |---|---|---|
/// | `content` | what a page is made of | nobody who keeps the page |
/// | `navigation` | document furniture pointing at pages: outlines, form field trees, article threads | a subsetting operation (ADR 0019 §1, §2b) |
/// | `page-tree` | an intermediate `/Pages` node, holding other pages | anything that moves a page (ADR 0021) |
///
/// So a subsetting operation requires `"content"`; `rotate` and `compress` require
/// `"content"` and `"navigation"`; `reorder` requires the same two and **states** that it
/// loses the third, by name.
///
/// **The third kind is an exemption, not a weakening.** `assert_nothing_lost` still requires
/// everything it is given, whole. The alternative considered and rejected was relaxing it to a
/// subset check, which would have quietly relaxed it for `rotate` and `compress` as well.
#[must_use]
pub fn owned_by(marked: &Marked, pages: &[u64], kinds: &[&str]) -> Vec<u64> {
    marked
        .objects
        .iter()
        .filter(|(_, object)| kinds.contains(&object.kind.as_str()))
        .filter(|(_, object)| object.pages.iter().any(|p| pages.contains(p)))
        .map(|(number, _)| *number)
        .collect()
}

/// Assert the closure property, reporting what was examined.
///
/// `CLAUDE.md`: a check reports what it examined and gates on the expected count where that is
/// derivable. Here it is -- the manifest knows how many objects there are -- so a scan that
/// found nothing because it stopped looking is distinguishable from a clean run.
/// **This asks one question only: did anything trespass.** It does not ask whether anything was
/// lost, and with every page included it is vacuous — see [`assert_nothing_lost`], which is what
/// a non-subsetting operation needs.
pub fn assert_closed(marked: &Marked, output: &[u8], included: &[u64], what: &str) {
    let survived = survivors(marked, output);
    println!(
        "  {what}: {} of {} marked objects survived ({} unmarkable), pages {:?}",
        survived.len(),
        marked.objects.len(),
        marked.unmarkable,
        included
    );
    assert!(
        !survived.is_empty(),
        "{what}: no marked object survived at all, so this assertion is about an empty set"
    );

    let trespassing = trespassers(marked, output, included);
    assert!(
        trespassing.is_empty(),
        "{what}: the output contains pages {included:?} and carries {} object(s) belonging \
         only to pages it excluded: {trespassing:?}",
        trespassing.len()
    );
}
