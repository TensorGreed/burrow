//! The fixtures and near-miss twins for flatten slices S5 to S7 (ADR 0031 §2 rules 1, 2, 2a, 3
//! and 4), held to differ in exactly the property each rule keys on.
//!
//! **What this asserts today, before the slices exist:** the structure of the fixtures themselves.
//! For each rule, a witness written here -- an oracle over the fixture's own object model, not the
//! slice's walk -- says the fixture has the property and its twin does not; and every document's
//! bytes are read back through `burrow_engines::pdfsyntax` to show they say what the model says.
//! Natively, every document also opens in qpdf with its page count.
//!
//! **What waits for the slice:** the refusal id each fixture produces, and `Ok` for each twin.
//! Nothing here is skipped or parked to look green; those assertions are listed in
//! `~/burrow-census/rounds/flatten-s5-s7-fixtures/PLAN.md` for the slices to add.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

mod flatten_fixtures;

use std::collections::{BTreeMap, BTreeSet};

use burrow_engines::pdfsyntax::{references, top_level_keys};
use flatten_fixtures::{
    ANNOTATION_SHAPES, AnnotationShape, CATALOG, Case, Doc, ENCODINGS, FORM_SHAPES, FormShape,
    NOT_ZLIB, PAGE_N, TREE_ENTRIES, ThumbnailShape, TreeEntry, V, XFA_SHAPES, XMP_NS, XfaShape,
    adler32, annotation, annotation_font_twin, claims_tags, form, metadata_thumbnail, packet,
    structure_kept_by_another_page, thumbnail_in_another_namespace, thumbnail_shapes,
    untyped_element_kept_by_outline, xfa,
};

// ---------------------------------------------------------------------------------------------
// The witnesses.
// ---------------------------------------------------------------------------------------------

/// Rule 1's tree members: everything reachable from the `/StructTreeRoot` by the walk that stops
/// at pages, not entering what the tree points at without owning (`/Pg`; the fixtures write no
/// `/Obj` or `/Stm`). With `by_type`, only those declaring a structure `/Type` -- the reading the
/// ADR's mutation plants, and the one the untyped fixture exists to defeat.
fn structure_members(doc: &Doc, by_type: bool) -> BTreeSet<u32> {
    structure_members_omitting(doc, by_type, None)
}

/// [`structure_members`], by a walk that also does not follow `omit`, wherever it sits: the walk a
/// slice would have if it left that entry out of its list.
fn structure_members_omitting(doc: &Doc, by_type: bool, omit: Option<&str>) -> BTreeSet<u32> {
    let Some(V::Ref(root)) = doc.value(CATALOG).get("StructTreeRoot") else {
        return BTreeSet::new();
    };
    let members = doc.walk(&[*root], &|_, key| {
        matches!(key, Some("Pg" | "Obj" | "Stm" | "StmOwn")) || (omit.is_some() && key == omit)
    });
    if !by_type {
        return members;
    }
    members
        .into_iter()
        .filter(|&n| {
            matches!(doc.value(n).get("Type"),
                Some(V::Name(t)) if t == "StructElem" || t == "StructTreeRoot")
        })
        .collect()
}

/// Rule 1's property: tree members the output still reaches once the catalogue loses
/// `/StructTreeRoot` and `/MarkInfo` and page N is replaced.
fn structure_kept(doc: &Doc, by_type: bool) -> BTreeSet<u32> {
    structure_kept_omitting(doc, by_type, None)
}

/// [`structure_kept`], over [`structure_members_omitting`]`(omit)`.
fn structure_kept_omitting(doc: &Doc, by_type: bool, omit: Option<&str>) -> BTreeSet<u32> {
    let kept = doc.kept(&["StructTreeRoot", "MarkInfo"]);
    structure_members_omitting(doc, by_type, omit)
        .intersection(&kept)
        .copied()
        .collect()
}

/// The key a [`TreeEntry`]'s member is reached through.
fn entry_key(entry: TreeEntry) -> &'static str {
    match entry {
        TreeEntry::Kids => "K",
        TreeEntry::ParentTree => "ParentTree",
        TreeEntry::IdTree => "IDTree",
        TreeEntry::RoleMap => "RoleMap",
        TreeEntry::ClassMap => "ClassMap",
        TreeEntry::Attributes => "A",
        TreeEntry::Namespaces => "Namespaces",
        TreeEntry::PronunciationLexicon => "PronunciationLexicon",
    }
}

/// Rule 2a's keys: the `/AcroForm`'s `/XFA` value, and whether the catalogue has `/NeedsRendering`
/// (the rule keys on presence, with any value).
fn xfa_keys(doc: &Doc) -> (Option<V>, bool) {
    let Some(V::Ref(acroform)) = doc.value(CATALOG).get("AcroForm") else {
        panic!("no AcroForm");
    };
    (
        doc.value(*acroform).get("XFA").cloned(),
        doc.value(CATALOG).get("NeedsRendering").is_some(),
    )
}

/// Every object a document reaches from its catalogue, following every reference.
fn reachable(doc: &Doc) -> BTreeSet<u32> {
    let mut seen = BTreeSet::new();
    let mut todo = vec![CATALOG];
    while let Some(n) = todo.pop() {
        if seen.insert(n) {
            todo.extend(doc.value(n).refs());
        }
    }
    seen
}

/// The annotations a page lists, through an indirect `/Annots` array or a direct one; and the
/// array's own object number when it is indirect.
fn annots_of(doc: &Doc, page: u32) -> (Vec<u32>, Option<u32>) {
    let (items, array) = match doc.value(page).get("Annots") {
        Some(V::Arr(items)) => (items.clone(), None),
        Some(V::Ref(n)) => match doc.value(*n) {
            V::Arr(items) => (items.clone(), Some(*n)),
            _ => (Vec::new(), Some(*n)),
        },
        _ => (Vec::new(), None),
    };
    let refs = items
        .iter()
        .filter_map(|v| match v {
            V::Ref(n) => Some(*n),
            _ => None,
        })
        .collect();
    (refs, array)
}

/// Every object in the `/AcroForm /Fields` tree, through `/Kids`.
fn fields_tree(doc: &Doc) -> BTreeSet<u32> {
    let Some(V::Ref(acroform)) = doc.value(CATALOG).get("AcroForm") else {
        return BTreeSet::new();
    };
    let mut todo: Vec<u32> = doc
        .value(*acroform)
        .get("Fields")
        .map(|v| v.refs().into_iter().collect())
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    while let Some(n) = todo.pop() {
        if seen.insert(n)
            && let Some(kids) = doc.value(n).get("Kids")
        {
            todo.extend(kids.refs());
        }
    }
    seen
}

/// Which of rule 2's three clauses `page` meets: 1, a listed annotation whose `/Subtype` is
/// `Widget` as a name or a string; 2, a listed annotation with `/FT` on itself or a `/Parent`
/// ancestor, or in the `/Fields` tree; 3, a `/Fields` tree object whose `/P` names the page.
fn form_clauses(doc: &Doc, page: u32) -> BTreeSet<u8> {
    let (annots, _) = annots_of(doc, page);
    let fields = fields_tree(doc);
    let mut clauses = BTreeSet::new();
    for &a in &annots {
        match doc.value(a).get("Subtype") {
            Some(V::Name(s)) if s == "Widget" => {
                clauses.insert(1);
            }
            Some(V::Str(s)) if s == b"Widget" => {
                clauses.insert(1);
            }
            _ => {}
        }
        let mut at = Some(a);
        for _ in 0..64 {
            let Some(n) = at else { break };
            if doc.value(n).get("FT").is_some() {
                clauses.insert(2);
            }
            at = match doc.value(n).get("Parent") {
                Some(V::Ref(p)) => Some(*p),
                _ => None,
            };
        }
        if fields.contains(&a) {
            clauses.insert(2);
        }
    }
    if fields
        .iter()
        .any(|&f| doc.value(f).get("P") == Some(&V::Ref(page)))
    {
        clauses.insert(3);
    }
    clauses
}

/// Rule 3's set *A*: page N's annotations, their `/Annots` array when it is indirect, and the
/// annotation-owned objects reachable from them by the walk that stops at pages, not entering an
/// appearance stream's `/Resources` (those are rule 5's). With `wide`, entering them -- the
/// widening the ADR's mutation plants, which the font twin exists to catch.
fn annotation_set(doc: &Doc, wide: bool) -> BTreeSet<u32> {
    let (annots, array) = annots_of(doc, PAGE_N);
    let mut roots = annots;
    roots.extend(array);
    doc.walk(&roots, &|object, key| {
        !wide && object.stream.is_some() && key == Some("Resources")
    })
}

/// Rule 3's property: members of *A* the output still reaches once page N is replaced.
fn annotation_kept(doc: &Doc, wide: bool) -> BTreeSet<u32> {
    let kept = doc.kept(&[]);
    annotation_set(doc, wide)
        .intersection(&kept)
        .copied()
        .collect()
}

/// The narrower wording round 0 rejected: a kept annotation whose `/IRT`, `/Parent` or `/Popup`
/// names one of page N's annotations.
fn annotation_kept_narrow(doc: &Doc) -> bool {
    let (on_n, _) = annots_of(doc, PAGE_N);
    let kept = doc.kept(&[]);
    kept.iter().any(|&k| {
        ["IRT", "Parent", "Popup"]
            .iter()
            .any(|key| matches!(doc.value(k).get(key), Some(V::Ref(r)) if on_n.contains(r)))
    })
}

/// Why rule 4's witness could not read a metadata stream.
#[derive(Debug, PartialEq, Eq)]
enum Unread {
    /// The stream's filter cannot start on its bytes.
    Undecodable,
    /// The packet is not well-formed XML.
    Unreadable,
}

/// A zlib stream's header passes its own check (RFC 1950, 2.2): deflate, and `CMF * 256 + FLG` a
/// multiple of 31.
fn zlib_header_ok(data: &[u8]) -> bool {
    data.len() >= 2
        && (u32::from(data[0]) * 256 + u32::from(data[1])).is_multiple_of(31)
        && data[0] & 0x0f == 8
}

/// Inflate a zlib stream of stored deflate blocks, the only kind `flatten_fixtures::zlib_stored` writes, checking
/// each block's length against its complement and the Adler-32 at the end.
fn inflate_stored(data: &[u8]) -> Vec<u8> {
    assert!(zlib_header_ok(data));
    let mut out = Vec::new();
    let mut at = 2;
    loop {
        let header = data[at];
        assert_eq!(header & 0b110, 0, "a stored block");
        let len = u16::from_le_bytes([data[at + 1], data[at + 2]]);
        let nlen = u16::from_le_bytes([data[at + 3], data[at + 4]]);
        assert_eq!(len, !nlen);
        at += 5;
        out.extend_from_slice(&data[at..at + usize::from(len)]);
        at += usize::from(len);
        if header & 1 == 1 {
            break;
        }
    }
    let check = u32::from_be_bytes(data[at..at + 4].try_into().unwrap());
    assert_eq!(check, adler32(&out));
    assert_eq!(at + 4, data.len(), "nothing after the checksum");
    out
}

/// The catalogue's metadata stream decoded to text: the encoding read from the first `<`, as XMP
/// part 1, 7.3.2 and XML's appendix F do. The only filter a fixture writes is `/FlateDecode`:
/// over bytes whose zlib header fails its own check ([`Unread::Undecodable`]), or over a valid
/// stream of stored blocks, which is inflated.
fn metadata_text(doc: &Doc) -> Result<String, Unread> {
    let Some(V::Ref(n)) = doc.value(CATALOG).get("Metadata") else {
        panic!("no metadata stream");
    };
    let object = doc.object(*n);
    let mut data = object.stream.clone().unwrap();
    if let Some(filter) = object.value.get("Filter") {
        assert_eq!(filter, &V::Name("FlateDecode".into()));
        if !zlib_header_ok(&data) {
            return Err(Unread::Undecodable);
        }
        data = inflate_stored(&data);
    }
    let data = data.as_slice();
    let units = |width: usize, big: bool| -> Vec<u32> {
        data.chunks(width)
            .map(|c| {
                c.iter().enumerate().fold(0u32, |acc, (i, &b)| {
                    let shift = if big { width - 1 - i } else { i } * 8;
                    acc | (u32::from(b) << shift)
                })
            })
            .collect()
    };
    let code_points = match data {
        [0, 0, 0, b'<', ..] => units(4, true),
        [b'<', 0, 0, 0, ..] => units(4, false),
        [0, b'<', ..] => units(2, true),
        [b'<', 0, ..] => units(2, false),
        _ => return Ok(String::from_utf8(data.to_vec()).unwrap()),
    };
    // Every packet the fixtures write is in the Basic Multilingual Plane, so a UTF-16 unit is its
    // code point.
    Ok(code_points
        .into_iter()
        .map(|c| char::from_u32(c).unwrap())
        .collect())
}

/// Whether the packet has a `Thumbnails` element in XMP's namespace, resolving each prefix through
/// the packet's own `xmlns` declarations (document-wide: the fixtures declare each once). With
/// `by_prefix`, matching the conventional `xmp:` prefix instead -- the reading the ADR's mutation
/// plants. A packet whose elements do not balance is [`Unread::Unreadable`].
fn has_thumbnail(xml: &str, by_prefix: bool) -> Result<bool, Unread> {
    let mut namespaces = BTreeMap::new();
    let mut rest = xml;
    while let Some(at) = rest.find("xmlns") {
        rest = &rest[at + 5..];
        let prefix = rest
            .strip_prefix(':')
            .map_or("", |r| &r[..r.find('=').unwrap()]);
        let open = rest.find('"').unwrap();
        let close = open + 1 + rest[open + 1..].find('"').unwrap();
        namespaces.insert(prefix.to_owned(), rest[open + 1..close].to_owned());
        rest = &rest[close..];
    }
    let mut stack = Vec::new();
    let mut found = false;
    let mut rest = xml;
    while let Some(at) = rest.find('<') {
        rest = &rest[at + 1..];
        let end = rest.find('>').ok_or(Unread::Unreadable)?;
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if tag.starts_with('?') {
            continue;
        }
        if let Some(closing) = tag.strip_prefix('/') {
            if stack.pop().as_deref() != Some(closing) {
                return Err(Unread::Unreadable);
            }
            continue;
        }
        let qualified = tag.split_whitespace().next().unwrap().trim_end_matches('/');
        let (prefix, local) = qualified.split_once(':').unwrap_or(("", qualified));
        if local == "Thumbnails" {
            found |= if by_prefix {
                prefix == "xmp"
            } else {
                namespaces.get(prefix).map(String::as_str) == Some(XMP_NS)
            };
        }
        if !tag.ends_with('/') {
            stack.push(qualified.to_owned());
        }
    }
    if !stack.is_empty() {
        return Err(Unread::Unreadable);
    }
    Ok(found)
}

/// Rule 4's property: the stream cannot be read (refuses), or it carries a thumbnail.
fn metadata_fires(doc: &Doc, by_prefix: bool) -> bool {
    match metadata_text(doc) {
        Err(_) => true,
        Ok(text) => has_thumbnail(&text, by_prefix).unwrap_or(true),
    }
}

// ---------------------------------------------------------------------------------------------
// Every case, for the read-back checks.
// ---------------------------------------------------------------------------------------------

fn every_case() -> Vec<Case> {
    let case = |name: String, doc: Doc, fires: bool| Case { name, doc, fires };
    let mut cases = Vec::new();
    for entry in TREE_ENTRIES {
        for kept in [true, false] {
            cases.push(case(
                format!("rule1/{entry:?}/{}", if kept { "fires" } else { "twin" }),
                structure_kept_by_another_page(entry, kept).0,
                kept,
            ));
        }
    }
    for kept in [true, false] {
        cases.push(case(
            format!("rule1/untyped/{kept}"),
            untyped_element_kept_by_outline(kept).0,
            kept,
        ));
        cases.push(case(format!("rule1/drop/{kept}"), claims_tags(kept), kept));
    }
    for shape in FORM_SHAPES {
        for on in [true, false] {
            cases.push(case(format!("rule2/{shape:?}/{on}"), form(shape, on), on));
        }
    }
    for shape in XFA_SHAPES {
        for on in [true, false] {
            cases.push(case(format!("rule2a/{shape:?}/{on}"), xfa(shape, on), on));
        }
    }
    for shape in ANNOTATION_SHAPES {
        for on in [true, false] {
            cases.push(case(
                format!("rule3/{shape:?}/{on}"),
                annotation(shape, on),
                on,
            ));
        }
    }
    cases.push(case(
        "rule3/font-twin".to_owned(),
        annotation_font_twin(),
        false,
    ));
    for shape in thumbnail_shapes() {
        for on in [true, false] {
            cases.push(case(
                format!("rule4/{shape:?}/{on}"),
                metadata_thumbnail(shape, on),
                on,
            ));
        }
    }
    cases.push(case(
        "rule4/other-namespace".to_owned(),
        thumbnail_in_another_namespace(),
        false,
    ));
    cases
}

/// The number of cases [`every_case`] builds: rule 1, 8 entries and the untyped and drop pairs; rule
/// 2, five shapes; rule 2a, five; rule 3, four and the font twin; rule 4, nine shapes and the
/// other-namespace twin. Gated on exactly, so a case lost from the list is a failure rather than a
/// smaller green.
const CASES: usize = 2 * 8 + 2 * 2 + 2 * 5 + 2 * 5 + 2 * 4 + 1 + 2 * 9 + 1;

// ---------------------------------------------------------------------------------------------
// The bytes say what the model says.
// ---------------------------------------------------------------------------------------------

/// Every object of every case, serialised, reads back through `pdfsyntax` with the model's keys and
/// the model's references, and the whole file scans as regular with one declaration per object.
/// The objects read back are counted, and that count is gated on the number the cases' catalogues
/// reach, derived from the models: an object no catalogue reaches (a stray one a builder added) is
/// a failure, not a larger total.
#[test]
fn every_fixture_reads_back_as_its_model() {
    let cases = every_case();
    assert_eq!(cases.len(), CASES, "cases built");
    let mut objects = 0;
    let mut declared = 0;
    let mut reached = 0;
    for case in &cases {
        let doc = &case.doc;
        let bytes = doc.bytes();
        let file = references::in_file(&bytes, &mut || Ok(())).unwrap();
        assert_eq!(
            file.irregular, None,
            "{}: the file scans regularly",
            case.name
        );
        assert_eq!(
            file.declarations.len(),
            doc.objects.len(),
            "{}: one declaration per object",
            case.name
        );
        declared += file.declarations.len();
        let reaches = reachable(doc).len();
        assert_eq!(
            reaches,
            file.declarations.len(),
            "{}: the catalogue reaches every object the file declares",
            case.name
        );
        reached += reaches;
        for n in 1..=u32::try_from(doc.objects.len()).unwrap() {
            let value = &doc.object(n).value;
            let found = references::in_value(&value.text(), &mut || Ok(())).unwrap();
            assert_eq!(found.irregular, None, "{} object {n}", case.name);
            let read: BTreeSet<u32> = found
                .references
                .iter()
                .map(|&(number, generation)| {
                    assert_eq!(generation, 0);
                    u32::try_from(number).unwrap()
                })
                .collect();
            assert_eq!(read, value.refs(), "{} object {n}: references", case.name);
            if matches!(value, V::Dict(_)) {
                let keys: Vec<Vec<u8>> =
                    value.keys().iter().map(|k| k.as_bytes().to_vec()).collect();
                assert_eq!(
                    top_level_keys(&value.text()).unwrap(),
                    keys,
                    "{} object {n}: keys",
                    case.name
                );
            }
            objects += 1;
        }
    }
    assert_eq!(objects, declared, "every declared object was read back");
    assert_eq!(
        objects, reached,
        "objects read back == objects the catalogues reach"
    );
    eprintln!(
        "read back {} cases, {objects} objects ({reached} reached from the catalogues)",
        cases.len()
    );
}

/// Every case's `fires` flag is what its rule's witness says of it: the flags are the record the
/// slices will assert refusals against, so they are held to the witnesses here rather than trusted.
#[test]
fn every_case_fires_as_its_rule_witness_says() {
    let cases = every_case();
    assert_eq!(cases.len(), CASES, "cases built");
    let mut per_rule = BTreeMap::new();
    let mut firing = 0;
    for case in &cases {
        let doc = &case.doc;
        let mut parts = case.name.split('/');
        let rule = parts.next().unwrap();
        let witness = match (rule, parts.next()) {
            ("rule1", Some("drop")) => doc.value(CATALOG).get("StructTreeRoot").is_some(),
            ("rule1", _) => !structure_kept(doc, false).is_empty(),
            ("rule2", _) => !form_clauses(doc, PAGE_N).is_empty(),
            ("rule2a", _) => {
                let (xfa_value, needs_rendering) = xfa_keys(doc);
                xfa_value.is_some() || needs_rendering
            }
            ("rule3", _) => !annotation_kept(doc, false).is_empty(),
            ("rule4", _) => metadata_fires(doc, false),
            (other, _) => panic!("{}: no witness for {other}", case.name),
        };
        assert_eq!(case.fires, witness, "{}: fires", case.name);
        *per_rule.entry(rule).or_insert(0) += 1;
        firing += usize::from(case.fires);
    }
    let examined: usize = per_rule.values().sum();
    assert_eq!(examined, CASES);
    eprintln!("{examined} cases held to their witness, {firing} fire: {per_rule:?}");
}

// ---------------------------------------------------------------------------------------------
// Rule 1.
// ---------------------------------------------------------------------------------------------

/// Each tree entry's fixture keeps its member reachable from the output, through another page, and
/// keeps no other tree member; a walk that leaves the entry out does not reach the member, so it
/// keeps nothing of that fixture's tree. Its twin is the same tree with no kept referrer, and keeps
/// none.
#[test]
fn rule_1_each_tree_entry_fixture_keeps_a_member_and_its_twin_does_not() {
    let mut examined = 0;
    for entry in TREE_ENTRIES {
        let (fixture, tree) = structure_kept_by_another_page(entry, true);
        let (twin, twin_tree) = structure_kept_by_another_page(entry, false);
        assert_ne!(fixture, twin, "{entry:?}: the twin differs");
        assert_eq!(tree.member, twin_tree.member);
        let members = structure_members(&fixture, false);
        assert!(
            members.contains(&tree.member),
            "{entry:?}: member in the tree"
        );
        assert!(members.contains(&tree.element));
        assert!(!members.contains(&PAGE_N), "the walk stops at pages");
        assert!(
            !structure_kept(&fixture, false).is_empty(),
            "{entry:?}: the fixture keeps a tree member"
        );
        assert_eq!(
            structure_kept(&fixture, false),
            BTreeSet::from([tree.member]),
            "{entry:?}: the member is the only tree member the fixture keeps"
        );
        let key = entry_key(entry);
        assert!(
            !structure_members_omitting(&fixture, false, Some(key)).contains(&tree.member),
            "{entry:?}: /{key} is the only route to the member"
        );
        assert_eq!(
            structure_kept_omitting(&fixture, false, Some(key)),
            BTreeSet::new(),
            "{entry:?}: a walk that leaves /{key} out keeps nothing of the fixture's tree"
        );
        assert_eq!(
            structure_kept(&twin, false),
            BTreeSet::new(),
            "{entry:?}: the twin keeps nothing of the tree"
        );
        assert_eq!(
            structure_members(&twin, false),
            members,
            "{entry:?}: same tree in both"
        );
        examined += 1;
    }
    assert_eq!(examined, TREE_ENTRIES.len());
}

/// The untyped fixture: nothing in its tree declares a structure `/Type`, a document-level object
/// keeps the element, and so membership by reachability sees it kept where membership by `/Type`
/// sees nothing. Its twin keeps nothing either way.
#[test]
fn rule_1_the_untyped_element_is_kept_only_by_reachability() {
    let (fixture, tree) = untyped_element_kept_by_outline(true);
    let (twin, _) = untyped_element_kept_by_outline(false);
    assert_eq!(fixture.value(tree.element).get("Type"), None);
    assert_eq!(fixture.value(tree.root).get("Type"), None);
    assert!(
        structure_kept(&fixture, false).contains(&tree.element),
        "kept, by reachability"
    );
    assert_eq!(
        structure_kept(&fixture, true),
        BTreeSet::new(),
        "a /Type-keyed reading sees nothing kept: the fixture is what defeats it"
    );
    assert_eq!(structure_kept(&twin, false), BTreeSet::new());
    assert_eq!(
        structure_members(&twin, false),
        structure_members(&fixture, false)
    );
}

/// Rule 1's drop: the tagged fixture's catalogue claims tags (`/MarkInfo`, `/StructTreeRoot`) and
/// its page has `/StructParents`; the untagged twin has none of the three.
#[test]
fn rule_1_the_tagged_document_claims_tags_and_its_twin_does_not() {
    let tagged = claims_tags(true);
    let untagged = claims_tags(false);
    for key in ["MarkInfo", "StructTreeRoot"] {
        assert!(tagged.value(CATALOG).get(key).is_some(), "{key}");
        assert!(untagged.value(CATALOG).get(key).is_none(), "{key}");
    }
    assert!(tagged.value(PAGE_N).get("StructParents").is_some());
    assert!(untagged.value(PAGE_N).get("StructParents").is_none());
}

// ---------------------------------------------------------------------------------------------
// Rule 2.
// ---------------------------------------------------------------------------------------------

/// Each form fixture meets exactly the clauses it is written for on page N, so dropping any one
/// clause leaves a fixture that only it catches; each twin, the same shape on page 1, meets none on
/// page N, and meets the same clauses on page 1.
#[test]
fn rule_2_each_form_fixture_meets_its_own_clauses_and_its_twin_none() {
    let expected = |shape| -> BTreeSet<u8> {
        match shape {
            FormShape::WidgetWithNoField | FormShape::WidgetSubtypeString => [1].into(),
            FormShape::FieldTypeOnAncestor => [2].into(),
            FormShape::FieldsEntryNamesPage => [3].into(),
            FormShape::SignatureField => [1, 2].into(),
        }
    };
    let mut sole = BTreeSet::new();
    for shape in FORM_SHAPES {
        let fixture = form(shape, true);
        let twin = form(shape, false);
        assert_eq!(form_clauses(&fixture, PAGE_N), expected(shape), "{shape:?}");
        assert_eq!(
            form_clauses(&twin, PAGE_N),
            BTreeSet::new(),
            "{shape:?}: twin on page N"
        );
        assert_eq!(
            form_clauses(&twin, twin.page(1)),
            expected(shape),
            "{shape:?}: twin, the same shape on page 1"
        );
        assert!(twin.value(CATALOG).get("AcroForm").is_some());
        if expected(shape).len() == 1 {
            sole.extend(expected(shape));
        }
    }
    assert_eq!(
        sole,
        [1, 2, 3].into(),
        "every clause has a fixture only it catches"
    );
    // The string-subtype fixture is a string, not a name.
    let string = form(FormShape::WidgetSubtypeString, true);
    let (annots, _) = annots_of(&string, PAGE_N);
    assert!(matches!(
        string.value(annots[0]).get("Subtype"),
        Some(V::Str(_))
    ));
    // The signature fixture is a signature field.
    let signature = form(FormShape::SignatureField, true);
    let (annots, _) = annots_of(&signature, PAGE_N);
    assert_eq!(
        signature.value(annots[0]).get("FT"),
        Some(&V::Name("Sig".into()))
    );
}

// ---------------------------------------------------------------------------------------------
// Rule 2a.
// ---------------------------------------------------------------------------------------------

/// Each XFA fixture has its key; the twin has neither key and the same `/AcroForm` otherwise. The
/// three `/XFA` fixtures hold three different kinds of value, and the two `/NeedsRendering`
/// fixtures both values, so a rule keyed on the parsed value misses at least one of each.
#[test]
fn rule_2a_each_xfa_fixture_has_its_key_and_the_twin_neither() {
    let mut kinds = BTreeSet::new();
    let mut rendering_values = Vec::new();
    for shape in XFA_SHAPES {
        let (xfa_value, needs_rendering) = xfa_keys(&xfa(shape, true));
        assert_eq!(
            xfa_keys(&xfa(shape, false)),
            (None, false),
            "{shape:?}: twin"
        );
        match shape {
            XfaShape::NeedsRendering | XfaShape::NeedsRenderingFalse => {
                assert!(needs_rendering && xfa_value.is_none(), "{shape:?}");
                rendering_values.push(
                    xfa(shape, true)
                        .value(CATALOG)
                        .get("NeedsRendering")
                        .cloned(),
                );
            }
            _ => {
                assert!(!needs_rendering);
                let doc = xfa(shape, true);
                kinds.insert(match xfa_value.unwrap() {
                    V::Arr(_) => "array",
                    V::Int(_) => "integer",
                    V::Ref(n) if doc.object(n).stream.is_some() => "stream",
                    other => panic!("{shape:?}: {other:?}"),
                });
            }
        }
    }
    assert_eq!(kinds, ["array", "integer", "stream"].into());
    assert_eq!(
        rendering_values,
        [Some(V::Bool(true)), Some(V::Bool(false))]
    );
}

// ---------------------------------------------------------------------------------------------
// Rule 3.
// ---------------------------------------------------------------------------------------------

/// Each annotation fixture keeps a member of *A* reachable from the output; each twin, the same
/// shape between two pages unrelated to page N, keeps none, though page N has an annotation of its
/// own in both. None of the fixtures is caught by the narrower `/IRT`, `/Parent`, `/Popup` wording.
#[test]
fn rule_3_each_annotation_fixture_keeps_a_member_of_a_and_its_twin_does_not() {
    for shape in ANNOTATION_SHAPES {
        let fixture = annotation(shape, true);
        let twin = annotation(shape, false);
        assert_ne!(
            annotation_kept(&fixture, false),
            BTreeSet::new(),
            "{shape:?}: fixture"
        );
        assert_eq!(
            annotation_kept(&twin, false),
            BTreeSet::new(),
            "{shape:?}: twin"
        );
        assert_eq!(
            annotation_kept(&twin, true),
            BTreeSet::new(),
            "{shape:?}: twin, wide"
        );
        assert!(
            !annotation_set(&twin, false).is_empty(),
            "{shape:?}: N has its own"
        );
        assert!(
            !annotation_kept_narrow(&fixture),
            "{shape:?}: only the widened rule sees it"
        );
    }
    // The shared array holds no annotation, so the array is the only member of A it keeps: an A
    // without page N's indirect /Annots array keeps nothing of this fixture.
    let shared = annotation(AnnotationShape::SharedAnnotsArray, true);
    let (on_n, array) = annots_of(&shared, PAGE_N);
    let array = array.expect("an indirect /Annots array");
    assert_eq!(on_n, Vec::<u32>::new(), "the shared array lists nothing");
    assert_eq!(
        annots_of(&shared, shared.page(1)),
        (Vec::new(), Some(array))
    );
    assert_eq!(annotation_kept(&shared, false), BTreeSet::from([array]));
}

/// The font twin: page N's annotation appearance uses a font kept page 1 also uses. *A*, which
/// stops at appearance resources, keeps nothing; *A* widened to them keeps the font.
#[test]
fn rule_3_the_font_twin_is_kept_only_through_appearance_resources() {
    let twin = annotation_font_twin();
    assert_eq!(annotation_kept(&twin, false), BTreeSet::new());
    let wide = annotation_kept(&twin, true);
    assert_eq!(wide.len(), 1);
    let font = *wide.iter().next().unwrap();
    assert_eq!(twin.value(font).get("Type"), Some(&V::Name("Font".into())));
}

// ---------------------------------------------------------------------------------------------
// Rule 4.
// ---------------------------------------------------------------------------------------------

/// Each thumbnail fixture fires (a thumbnail in XMP's namespace, or a stream that cannot be read)
/// and each twin does not. In every encoding but UTF-8 the property's name is not in the raw bytes,
/// so a byte search misses it.
#[test]
fn rule_4_each_thumbnail_fixture_fires_and_its_twin_does_not() {
    let shapes = thumbnail_shapes();
    assert_eq!(shapes.len(), ENCODINGS.len() + 4);
    for shape in shapes {
        let fixture = metadata_thumbnail(shape, true);
        let twin = metadata_thumbnail(shape, false);
        assert_ne!(fixture, twin, "{shape:?}");
        assert!(metadata_fires(&fixture, false), "{shape:?}: fixture");
        assert!(!metadata_fires(&twin, false), "{shape:?}: twin");
        match shape {
            ThumbnailShape::Undecodable => {
                assert_eq!(metadata_text(&fixture), Err(Unread::Undecodable));
                let n = fixture.value(CATALOG).get("Metadata").unwrap().refs();
                let n = *n.iter().next().unwrap();
                assert_eq!(fixture.object(n).stream.as_deref(), Some(NOT_ZLIB));
                // The twin is the same filter over a valid zlib stream of the plain packet: a
                // reading that refuses on the filter, not on the failure to decode, fires on it.
                let t = *twin
                    .value(CATALOG)
                    .get("Metadata")
                    .unwrap()
                    .refs()
                    .iter()
                    .next()
                    .unwrap();
                assert_eq!(
                    twin.object(t).value.get("Filter"),
                    fixture.object(n).value.get("Filter")
                );
                assert!(zlib_header_ok(twin.object(t).stream.as_deref().unwrap()));
                assert_eq!(metadata_text(&twin), Ok(packet("xmp", XMP_NS, false)));
            }
            ThumbnailShape::Unreadable => {
                let text = metadata_text(&fixture).unwrap();
                assert_eq!(has_thumbnail(&text, false), Err(Unread::Unreadable));
            }
            ThumbnailShape::Plain(encoding) => {
                let data = metadata_text(&fixture).unwrap();
                assert!(has_thumbnail(&data, false).unwrap());
                let n = *fixture
                    .value(CATALOG)
                    .get("Metadata")
                    .unwrap()
                    .refs()
                    .iter()
                    .next()
                    .unwrap();
                let raw = fixture.object(n).stream.clone().unwrap();
                let ascii = raw.windows(10).any(|w| w == b"Thumbnails");
                assert_eq!(
                    ascii,
                    encoding == flatten_fixtures::Encoding::Utf8,
                    "{encoding:?}: the name is in the raw bytes only in UTF-8"
                );
            }
            ThumbnailShape::ReboundPrefix | ThumbnailShape::DefaultNamespace => {
                assert!(
                    !metadata_fires(&fixture, true),
                    "{shape:?}: a prefix-keyed reading misses it"
                );
            }
        }
    }
}

/// The other-namespace twin carries `xmp:Thumbnails` bound to a namespace that is not XMP's: the
/// namespace-keyed reading does not fire on it, and a prefix-keyed reading does.
#[test]
fn rule_4_the_other_namespace_twin_fires_only_when_keyed_on_the_prefix() {
    let twin = thumbnail_in_another_namespace();
    assert!(!metadata_fires(&twin, false));
    assert!(metadata_fires(&twin, true));
}

// ---------------------------------------------------------------------------------------------
// Natively: every case opens in qpdf.
// ---------------------------------------------------------------------------------------------

/// Every fixture and twin opens in qpdf with the model's page count, so the slices meet them as
/// documents rather than as open failures.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#[test]
fn every_fixture_opens_in_qpdf_with_its_page_count() {
    use std::sync::Arc;

    use burrow_engines::qpdf::Qpdf;
    use burrow_engines::{OpenOptions, PageRotator};
    use burrow_types::{Clock, Limits, ManualClock};

    let options = OpenOptions::new(
        Limits::DEFAULT,
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    );
    let engine = Qpdf::new();
    let cases = every_case();
    assert_eq!(cases.len(), CASES);
    for case in &cases {
        let source = PageRotator::open(&engine, case.doc.bytes().into_boxed_slice(), &options)
            .unwrap_or_else(|e| panic!("{}: {e:?}", case.name));
        assert_eq!(
            engine.pages(&source).unwrap(),
            u64::try_from(case.doc.pages().len()).unwrap(),
            "{}",
            case.name
        );
    }
    eprintln!("qpdf opened {} cases", cases.len());
}
