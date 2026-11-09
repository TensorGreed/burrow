//! Fixtures and near-miss twins for redaction v1's flatten slices S5 to S7 (ADR 0031 §2 rules 1,
//! 2, 2a, 3 and 4, and its *Fixtures, by rule* table).
//!
//! **Test-only, and built before the code that will consume them** (owner, decision 96). Each
//! fixture is a [`Doc`]: a small object model that serialises to a well-formed PDF with an exact
//! cross-reference table, so the slice can open it without a repair. The model is what the
//! witnesses in `flatten_s5_s7_fixtures.rs` reason over; that file also checks the serialised
//! bytes say what the model says, through `burrow_engines::pdfsyntax`.
//!
//! What this module does **not** do is decide any rule. The witnesses are oracles over the fixture's
//! own structure -- what makes a fixture and its twin differ -- and say nothing about whether the
//! slice's walk agrees. That half (the refusal id each fixture produces) waits for the slice, and is
//! listed in `~/burrow-census/rounds/flatten-s5-s7-fixtures/PLAN.md`, not asserted here.
//!
//! In every document the redacted page is page 0 (`N`), object [`PAGE_N`].

#![allow(
    dead_code,
    reason = "each test binary that includes this uses a subset"
)]

use std::collections::BTreeSet;
use std::fmt::Write as _;

/// The catalogue's object number in every fixture.
pub const CATALOG: u32 = 1;
/// The page tree root's object number in every fixture.
pub const PAGES: u32 = 2;
/// The redacted page's object number in every fixture.
pub const PAGE_N: u32 = 3;

/// A PDF value, as the fixture writes it.
#[derive(Debug, Clone, PartialEq)]
pub enum V {
    /// `N 0 R`.
    Ref(u32),
    /// `/name`, the slash not included.
    Name(String),
    /// An integer.
    Int(i64),
    /// `true` or `false`.
    Bool(bool),
    /// A literal string, escaped on write.
    Str(Vec<u8>),
    /// `[ ... ]`.
    Arr(Vec<V>),
    /// `<< ... >>`, keys without their slash, in order.
    Dict(Vec<(String, V)>),
    /// `null`.
    Null,
}

/// `/name`.
pub fn name(s: &str) -> V {
    V::Name(s.to_owned())
}

/// A dictionary from `(key, value)` pairs.
pub fn dict(entries: Vec<(&str, V)>) -> V {
    V::Dict(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

impl V {
    /// The value under `key`, when this is a dictionary that has it.
    pub fn get(&self, key: &str) -> Option<&V> {
        match self {
            V::Dict(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The dictionary's keys, in order; empty for anything else.
    pub fn keys(&self) -> Vec<&str> {
        match self {
            V::Dict(entries) => entries.iter().map(|(k, _)| k.as_str()).collect(),
            _ => Vec::new(),
        }
    }

    /// Every object this value names, with the **top-level** key of the holding object under
    /// which it sits (`None` where the object's value is not a dictionary): a page's font under
    /// `/Resources /Font /F1` is recorded under `Resources`.
    fn references<'a>(&'a self, key: Option<&'a str>, out: &mut Vec<(Option<&'a str>, u32)>) {
        match self {
            V::Ref(n) => out.push((key, *n)),
            V::Arr(items) => items.iter().for_each(|v| v.references(key, out)),
            V::Dict(entries) => entries
                .iter()
                .for_each(|(k, v)| v.references(key.or(Some(k)), out)),
            _ => {}
        }
    }

    /// Every object number this value names, wherever it sits.
    pub fn refs(&self) -> BTreeSet<u32> {
        let mut out = Vec::new();
        self.references(None, &mut out);
        out.into_iter().map(|(_, n)| n).collect()
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            V::Ref(n) => out.extend_from_slice(format!("{n} 0 R").as_bytes()),
            V::Name(s) => out.extend_from_slice(format!("/{s}").as_bytes()),
            V::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            V::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            V::Str(bytes) => {
                out.push(b'(');
                for &b in bytes {
                    if matches!(b, b'(' | b')' | b'\\') {
                        out.push(b'\\');
                    }
                    out.push(b);
                }
                out.push(b')');
            }
            V::Arr(items) => {
                out.push(b'[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b' ');
                    }
                    v.write(out);
                }
                out.push(b']');
            }
            V::Dict(entries) => {
                out.extend_from_slice(b"<<");
                for (k, v) in entries {
                    out.extend_from_slice(format!(" /{k} ").as_bytes());
                    v.write(out);
                }
                out.extend_from_slice(b" >>");
            }
            V::Null => out.extend_from_slice(b"null"),
        }
    }

    /// The value's text, as written into the file.
    pub fn text(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }
}

/// One indirect object: a value, and stream data when the value is a stream's dictionary.
#[derive(Debug, Clone, PartialEq)]
pub struct Obj {
    /// The object's value; a stream's dictionary, without `/Length`, which is written for it.
    pub value: V,
    /// The stream's bytes, for a stream object.
    pub stream: Option<Vec<u8>>,
}

/// A fixture document: objects numbered from 1, the catalogue first.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    /// Object `i + 1` is `objects[i]`.
    pub objects: Vec<Obj>,
}

impl Doc {
    /// A document of `pages` pages, each drawing one line and nothing else. Objects 1 and 2 are
    /// the catalogue and the page tree, pages are 3.., and each page's content stream follows.
    pub fn with_pages(pages: u32) -> Self {
        let mut doc = Doc {
            objects: Vec::new(),
        };
        doc.add(dict(vec![
            ("Type", name("Catalog")),
            ("Pages", V::Ref(PAGES)),
        ]));
        doc.add(V::Null);
        let kids: Vec<u32> = (0..pages).map(|_| doc.add(V::Null)).collect();
        for &page in &kids {
            let contents = doc.add_stream(dict(vec![]), b"0 0 m 10 10 l S".to_vec());
            doc.set(
                page,
                dict(vec![
                    ("Type", name("Page")),
                    ("Parent", V::Ref(PAGES)),
                    (
                        "MediaBox",
                        V::Arr(vec![V::Int(0), V::Int(0), V::Int(200), V::Int(200)]),
                    ),
                    ("Resources", dict(vec![])),
                    ("Contents", V::Ref(contents)),
                ]),
            );
        }
        doc.set(
            PAGES,
            dict(vec![
                ("Type", name("Pages")),
                ("Kids", V::Arr(kids.iter().map(|&k| V::Ref(k)).collect())),
                ("Count", V::Int(i64::from(pages))),
            ]),
        );
        doc
    }

    /// The page objects, in page order.
    pub fn pages(&self) -> Vec<u32> {
        match self.value(PAGES).get("Kids") {
            Some(V::Arr(kids)) => kids
                .iter()
                .filter_map(|k| match k {
                    V::Ref(n) => Some(*n),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Page `index`'s object number.
    pub fn page(&self, index: usize) -> u32 {
        self.pages()[index]
    }

    /// Append an object; its number.
    pub fn add(&mut self, value: V) -> u32 {
        self.objects.push(Obj {
            value,
            stream: None,
        });
        u32::try_from(self.objects.len()).unwrap()
    }

    /// Append a stream object; its number.
    pub fn add_stream(&mut self, value: V, data: Vec<u8>) -> u32 {
        self.objects.push(Obj {
            value,
            stream: Some(data),
        });
        u32::try_from(self.objects.len()).unwrap()
    }

    /// Replace object `n`'s value.
    pub fn set(&mut self, n: u32, value: V) {
        self.objects[n as usize - 1].value = value;
    }

    /// Object `n`'s value.
    pub fn value(&self, n: u32) -> &V {
        &self.objects[n as usize - 1].value
    }

    /// Object `n`.
    pub fn object(&self, n: u32) -> &Obj {
        &self.objects[n as usize - 1]
    }

    /// Put `key` into object `n`'s dictionary, replacing any value it had.
    pub fn put(&mut self, n: u32, key: &str, value: V) {
        let V::Dict(entries) = &mut self.objects[n as usize - 1].value else {
            panic!("object {n} is not a dictionary");
        };
        entries.retain(|(k, _)| k != key);
        entries.push((key.to_owned(), value));
    }

    /// Append `item` to the array under `key` in object `n`, creating it if absent.
    pub fn push_to(&mut self, n: u32, key: &str, item: V) {
        let mut items = match self.value(n).get(key) {
            Some(V::Arr(items)) => items.clone(),
            _ => Vec::new(),
        };
        items.push(item);
        self.put(n, key, V::Arr(items));
    }

    /// Put `key` into the dictionary held at `outer` in object `n` (one level of nesting, such as a
    /// page's `/Resources`), creating that dictionary if absent.
    pub fn put_nested(&mut self, n: u32, outer: &str, key: &str, value: V) {
        let mut inner = match self.value(n).get(outer) {
            Some(V::Dict(entries)) => entries.clone(),
            _ => Vec::new(),
        };
        inner.retain(|(k, _)| k != key);
        inner.push((key.to_owned(), value));
        self.put(n, outer, V::Dict(inner));
    }

    /// The object's serialised text between `obj` and `endobj`.
    pub fn object_text(&self, n: u32) -> Vec<u8> {
        let object = self.object(n);
        match &object.stream {
            None => object.value.text(),
            Some(data) => {
                let mut value = object.value.clone();
                if let V::Dict(entries) = &mut value {
                    entries.push((
                        "Length".to_owned(),
                        V::Int(i64::try_from(data.len()).unwrap()),
                    ));
                }
                let mut out = value.text();
                out.extend_from_slice(b"\nstream\n");
                out.extend_from_slice(data);
                out.extend_from_slice(b"\nendstream");
                out
            }
        }
    }

    /// The document as a PDF with an exact classic cross-reference table and a binary comment.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::new();
        for n in 1..=self.objects.len() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{n} 0 obj\n").as_bytes());
            out.extend_from_slice(&self.object_text(u32::try_from(n).unwrap()));
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        let size = self.objects.len() + 1;
        let mut table = format!("xref\n0 {size}\n0000000000 65535 f \n");
        for offset in offsets {
            writeln!(table, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            table,
            "trailer\n<< /Size {size} /Root {CATALOG} 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        )
        .unwrap();
        out.extend_from_slice(table.as_bytes());
        out
    }

    /// The page-tree members by identity (ADR 0031 §2, *A walk that stops at pages*): the
    /// catalogue, and everything reached from its `/Pages` through `/Kids`.
    pub fn page_tree_members(&self) -> BTreeSet<u32> {
        let mut members = BTreeSet::from([CATALOG]);
        let mut todo = vec![PAGES];
        while let Some(n) = todo.pop() {
            if members.insert(n)
                && let Some(V::Arr(kids)) = self.value(n).get("Kids")
            {
                todo.extend(kids.iter().filter_map(|k| match k {
                    V::Ref(k) => Some(*k),
                    _ => None,
                }));
            }
        }
        members
    }

    /// Everything reachable from `roots` by the walk that stops at pages: it does not enter a
    /// page-tree member or the catalogue (they are not in the result), and it does not follow a
    /// reference whose top-level key `skip` names for the object holding it. The roots themselves are
    /// in the result unless they are members.
    pub fn walk(&self, roots: &[u32], skip: &dyn Fn(&Obj, Option<&str>) -> bool) -> BTreeSet<u32> {
        let stop = self.page_tree_members();
        let mut seen = BTreeSet::new();
        let mut todo: Vec<u32> = roots.to_vec();
        while let Some(n) = todo.pop() {
            if stop.contains(&n) || !seen.insert(n) {
                continue;
            }
            let object = self.object(n);
            let mut refs = Vec::new();
            object.value.references(None, &mut refs);
            todo.extend(
                refs.into_iter()
                    .filter(|(key, _)| !skip(object, *key))
                    .map(|(_, r)| r),
            );
        }
        seen
    }

    /// What the output can still reach once page N is replaced and the catalogue loses
    /// `removed`: everything reachable from the catalogue, entering the page tree, but not entering
    /// page N (its replacement keeps none of its references but `/Parent`, which the tree reaches
    /// anyway). This is the fixture's own model of "kept", for the witnesses; the slice's is its
    /// own.
    pub fn kept(&self, removed: &[&str]) -> BTreeSet<u32> {
        let mut seen = BTreeSet::new();
        let mut todo = vec![CATALOG];
        while let Some(n) = todo.pop() {
            if !seen.insert(n) || n == PAGE_N {
                continue;
            }
            let mut refs = Vec::new();
            self.object(n).value.references(None, &mut refs);
            todo.extend(
                refs.into_iter()
                    .filter(|(key, _)| !(n == CATALOG && key.is_some_and(|k| removed.contains(&k))))
                    .map(|(_, r)| r),
            );
        }
        seen.remove(&PAGE_N);
        seen
    }
}

/// A named fixture or twin.
pub struct Case {
    /// The case's name, `<rule>/<shape>[/twin]`.
    pub name: String,
    /// The document.
    pub doc: Doc,
    /// Whether the ADR says this case fires its rule.
    pub fires: bool,
}

// ---------------------------------------------------------------------------------------------
// Rule 1, `[structure-tree-kept]`.
// ---------------------------------------------------------------------------------------------

/// The structure tree's own entries a member can be reached through (ADR 0031 rule 1: "its kids,
/// its parent tree, its ID tree, its role and class maps, its attribute and namespace objects,
/// PDF 2.0's additions among them"). The slice writes the final list; each entry here has a
/// fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeEntry {
    /// `/K`: a marked-content reference, the kid of an element only `/StructTreeRoot /K` reaches.
    Kids,
    /// `/StructTreeRoot /ParentTree`: an empty leaf of the number tree.
    ParentTree,
    /// `/StructTreeRoot /IDTree`: an empty leaf of the name tree.
    IdTree,
    /// `/StructTreeRoot /RoleMap`, an indirect role map.
    RoleMap,
    /// `/StructTreeRoot /ClassMap`, an indirect class map.
    ClassMap,
    /// An element's `/A`, an attribute object.
    Attributes,
    /// `/StructTreeRoot /Namespaces` (PDF 2.0), a namespace dictionary.
    Namespaces,
    /// `/StructTreeRoot /PronunciationLexicon` (PDF 2.0), a lexicon stream.
    PronunciationLexicon,
}

/// Every [`TreeEntry`].
pub const TREE_ENTRIES: [TreeEntry; 8] = [
    TreeEntry::Kids,
    TreeEntry::ParentTree,
    TreeEntry::IdTree,
    TreeEntry::RoleMap,
    TreeEntry::ClassMap,
    TreeEntry::Attributes,
    TreeEntry::Namespaces,
    TreeEntry::PronunciationLexicon,
];

/// Object numbers in a structure-tree fixture.
pub struct Tree {
    /// The `/StructTreeRoot` object.
    pub root: u32,
    /// The element on page N.
    pub element: u32,
    /// The member reached through the entry under test.
    pub member: u32,
}

/// A tagged two-page document: `/MarkInfo << /Marked true >>`, a `/StructTreeRoot` with one
/// element on page N (`/Pg`, a marked-content kid, `/StructParents 0` on the page), and a member
/// reached only through `entry` that names nothing else in the tree. `untyped` leaves `/Type` off the element **and the root**, so that
/// nothing a kept untyped element reaches back to (its `/P`) declares a structure type either.
pub fn tagged(entry: TreeEntry, untyped: bool) -> (Doc, Tree) {
    let mut doc = Doc::with_pages(2);
    let root = doc.add(V::Null);
    let mut element = vec![
        ("S", name("P")),
        ("P", V::Ref(root)),
        ("Pg", V::Ref(PAGE_N)),
        ("K", V::Int(0)),
    ];
    if !untyped {
        element.insert(0, ("Type", name("StructElem")));
    }
    let element = doc.add(dict(element));
    let parent_tree = doc.add(dict(vec![(
        "Nums",
        V::Arr(vec![V::Int(0), V::Arr(vec![V::Ref(element)])]),
    )]));
    // Each member is reached through its own entry and no other, and names nothing else in the
    // tree, so a fixture's kept tree members are exactly {member}: a walk that omits the entry
    // keeps nothing of the fixture's tree, and would `Ok` it.
    let mut kids = vec![V::Ref(element)];
    let mut parent_tree_root = parent_tree;
    let mut root_dict = Vec::new();
    let member = match entry {
        TreeEntry::Kids => {
            // A second element, in no parent tree (its content is a marked-content reference,
            // not an MCID the page's `/StructParents` maps), reached only through `/K`; the member
            // is its kid, which names only page N.
            let reference = doc.add(dict(vec![
                ("Type", name("MCR")),
                ("Pg", V::Ref(PAGE_N)),
                ("MCID", V::Int(1)),
            ]));
            let mut division = vec![
                ("S", name("Div")),
                ("P", V::Ref(root)),
                ("K", V::Arr(vec![V::Ref(reference)])),
            ];
            if !untyped {
                division.insert(0, ("Type", name("StructElem")));
            }
            let division = doc.add(dict(division));
            kids.push(V::Ref(division));
            reference
        }
        TreeEntry::ParentTree => {
            // The number tree as a root over two leaves; the member is a leaf with no entries.
            doc.put(parent_tree, "Limits", V::Arr(vec![V::Int(0), V::Int(0)]));
            let empty = doc.add(dict(vec![("Nums", V::Arr(vec![]))]));
            parent_tree_root = doc.add(dict(vec![(
                "Kids",
                V::Arr(vec![V::Ref(parent_tree), V::Ref(empty)]),
            )]));
            empty
        }
        TreeEntry::IdTree => {
            // The name tree as a root over two leaves; the member is a leaf with no entries.
            let leaf = doc.add(dict(vec![
                (
                    "Limits",
                    V::Arr(vec![V::Str(b"e1".to_vec()), V::Str(b"e1".to_vec())]),
                ),
                (
                    "Names",
                    V::Arr(vec![V::Str(b"e1".to_vec()), V::Ref(element)]),
                ),
            ]));
            let empty = doc.add(dict(vec![("Names", V::Arr(vec![]))]));
            let ids = doc.add(dict(vec![(
                "Kids",
                V::Arr(vec![V::Ref(leaf), V::Ref(empty)]),
            )]));
            root_dict.push(("IDTree", V::Ref(ids)));
            empty
        }
        TreeEntry::RoleMap => {
            let map = doc.add(dict(vec![("Para", name("P"))]));
            root_dict.push(("RoleMap", V::Ref(map)));
            map
        }
        TreeEntry::ClassMap => {
            let map = doc.add(dict(vec![(
                "c1",
                dict(vec![("O", name("Layout")), ("Placement", name("Block"))]),
            )]));
            root_dict.push(("ClassMap", V::Ref(map)));
            map
        }
        TreeEntry::Attributes => {
            let attributes = doc.add(dict(vec![
                ("O", name("Layout")),
                ("Placement", name("Block")),
            ]));
            doc.put(element, "A", V::Ref(attributes));
            attributes
        }
        TreeEntry::Namespaces => {
            let namespace = doc.add(dict(vec![
                ("Type", name("Namespace")),
                ("NS", V::Str(b"http://iso.org/pdf2/ssn".to_vec())),
            ]));
            root_dict.push(("Namespaces", V::Arr(vec![V::Ref(namespace)])));
            namespace
        }
        TreeEntry::PronunciationLexicon => {
            let lexicon = doc.add_stream(
                dict(vec![]),
                b"<?xml version=\"1.0\"?><lexicon version=\"1.0\"/>".to_vec(),
            );
            root_dict.push(("PronunciationLexicon", V::Arr(vec![V::Ref(lexicon)])));
            lexicon
        }
    };
    let mut head = vec![
        ("K", V::Arr(kids)),
        ("ParentTree", V::Ref(parent_tree_root)),
        ("ParentTreeNextKey", V::Int(1)),
    ];
    if !untyped {
        head.insert(0, ("Type", name("StructTreeRoot")));
    }
    head.extend(root_dict);
    let root_dict = head;
    doc.set(root, dict(root_dict));
    doc.put(CATALOG, "StructTreeRoot", V::Ref(root));
    doc.put(CATALOG, "MarkInfo", dict(vec![("Marked", V::Bool(true))]));
    doc.put(PAGE_N, "StructParents", V::Int(0));
    (
        doc,
        Tree {
            root,
            element,
            member,
        },
    )
}

/// Rule 1's fixture for `entry`, and its twin: the same tree, with page 1 (a kept page) naming
/// the member from its marked-content property list in the fixture, and not in the twin.
pub fn structure_kept_by_another_page(entry: TreeEntry, kept_referrer: bool) -> (Doc, Tree) {
    let (mut doc, tree) = tagged(entry, false);
    if kept_referrer {
        let page = doc.page(1);
        doc.put_nested(
            page,
            "Resources",
            "Properties",
            dict(vec![("MC0", V::Ref(tree.member))]),
        );
    }
    (doc, tree)
}

/// Rule 1's untyped fixture and its twin: the element has no `/Type`, and a document-level object
/// (an outline item's `/SE`) keeps it in the fixture; the twin's outline item has no `/SE`.
pub fn untyped_element_kept_by_outline(kept_referrer: bool) -> (Doc, Tree) {
    let (mut doc, tree) = tagged(TreeEntry::Kids, true);
    let outlines = doc.add(V::Null);
    let mut item = vec![
        ("Title", V::Str(b"Section".to_vec())),
        ("Parent", V::Ref(outlines)),
        ("Dest", V::Arr(vec![V::Ref(doc.page(1)), name("Fit")])),
    ];
    if kept_referrer {
        item.push(("SE", V::Ref(tree.element)));
    }
    let item = doc.add(dict(item));
    doc.set(
        outlines,
        dict(vec![
            ("Type", name("Outlines")),
            ("First", V::Ref(item)),
            ("Last", V::Ref(item)),
            ("Count", V::Int(1)),
        ]),
    );
    doc.put(CATALOG, "Outlines", V::Ref(outlines));
    (doc, tree)
}

/// Rule 1's drop: a tagged document claiming tags, and its untagged twin.
pub fn claims_tags(tagged_document: bool) -> Doc {
    if tagged_document {
        tagged(TreeEntry::Kids, false).0
    } else {
        Doc::with_pages(2)
    }
}

// ---------------------------------------------------------------------------------------------
// Rule 2, `[form-field-on-page]`.
// ---------------------------------------------------------------------------------------------

/// The shapes rule 2's three clauses are written for (ADR 0031 rule 2, *Fixtures* row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormShape {
    /// A `/Subtype /Widget` annotation no field claims (clause 1 only).
    WidgetWithNoField,
    /// A widget whose `/Subtype` is the string `(Widget)`, no field claiming it (clause 1 only).
    WidgetSubtypeString,
    /// A field kid with no `/Subtype` and no `/FT` of its own, whose `/Parent` carries `/FT`, and
    /// which is not in the `/Fields` tree (clause 2 only).
    FieldTypeOnAncestor,
    /// A field in `/Fields` whose `/P` names the page, listed in no `/Annots` (clause 3 only).
    FieldsEntryNamesPage,
    /// A signature field widget, listed in `/Fields` (clauses 1 and 2).
    SignatureField,
}

/// Every [`FormShape`].
pub const FORM_SHAPES: [FormShape; 5] = [
    FormShape::WidgetWithNoField,
    FormShape::WidgetSubtypeString,
    FormShape::FieldTypeOnAncestor,
    FormShape::FieldsEntryNamesPage,
    FormShape::SignatureField,
];

/// Rule 2's fixture for `shape` (on page N) and its twin (the same shape on page 1, so every
/// widget of every field is on another page). Both documents have an `/AcroForm`.
pub fn form(shape: FormShape, on_page_n: bool) -> Doc {
    let mut doc = Doc::with_pages(2);
    let page = if on_page_n { PAGE_N } else { doc.page(1) };
    let rect = || V::Arr(vec![V::Int(10), V::Int(10), V::Int(60), V::Int(30)]);
    let mut fields = Vec::new();
    match shape {
        FormShape::WidgetWithNoField => {
            let widget = doc.add(dict(vec![
                ("Type", name("Annot")),
                ("Subtype", name("Widget")),
                ("Rect", rect()),
                ("P", V::Ref(page)),
            ]));
            doc.push_to(page, "Annots", V::Ref(widget));
        }
        FormShape::WidgetSubtypeString => {
            let widget = doc.add(dict(vec![
                ("Type", name("Annot")),
                ("Subtype", V::Str(b"Widget".to_vec())),
                ("Rect", rect()),
                ("P", V::Ref(page)),
            ]));
            doc.push_to(page, "Annots", V::Ref(widget));
        }
        FormShape::FieldTypeOnAncestor => {
            let parent = doc.add(V::Null);
            let kid = doc.add(dict(vec![
                ("Type", name("Annot")),
                ("Rect", rect()),
                ("Parent", V::Ref(parent)),
            ]));
            doc.set(
                parent,
                dict(vec![("FT", name("Tx")), ("T", V::Str(b"name".to_vec()))]),
            );
            doc.push_to(page, "Annots", V::Ref(kid));
        }
        FormShape::FieldsEntryNamesPage => {
            let field = doc.add(dict(vec![
                ("FT", name("Tx")),
                ("T", V::Str(b"name".to_vec())),
                ("P", V::Ref(page)),
            ]));
            fields.push(V::Ref(field));
        }
        FormShape::SignatureField => {
            let signature = doc.add(dict(vec![
                ("Type", name("Annot")),
                ("Subtype", name("Widget")),
                ("FT", name("Sig")),
                ("T", V::Str(b"signature".to_vec())),
                ("Rect", rect()),
            ]));
            doc.push_to(page, "Annots", V::Ref(signature));
            fields.push(V::Ref(signature));
        }
    }
    let acroform = doc.add(dict(vec![("Fields", V::Arr(fields))]));
    doc.put(CATALOG, "AcroForm", V::Ref(acroform));
    doc
}

// ---------------------------------------------------------------------------------------------
// Rule 2a, `[xfa-form]`.
// ---------------------------------------------------------------------------------------------

/// The ways rule 2a's keys are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfaShape {
    /// `/AcroForm /XFA` as an array of packet names and streams.
    Array,
    /// `/AcroForm /XFA` as one stream.
    Stream,
    /// `/AcroForm /XFA` as an integer: no XFA reader would take it, and the key is present.
    Integer,
    /// `/NeedsRendering true` on the catalogue, with no `/XFA`.
    NeedsRendering,
    /// `/NeedsRendering false` on the catalogue, with no `/XFA`: the rule keys on the key's
    /// presence, "with any value" (ADR 0031 rule 2a).
    NeedsRenderingFalse,
}

/// Every [`XfaShape`].
pub const XFA_SHAPES: [XfaShape; 5] = [
    XfaShape::Array,
    XfaShape::Stream,
    XfaShape::Integer,
    XfaShape::NeedsRendering,
    XfaShape::NeedsRenderingFalse,
];

/// Rule 2a's fixture for `shape`, or (with `xfa_present` false) its twin: the same `/AcroForm`
/// with neither key.
pub fn xfa(shape: XfaShape, xfa_present: bool) -> Doc {
    let mut doc = Doc::with_pages(2);
    let acroform = doc.add(dict(vec![("Fields", V::Arr(vec![]))]));
    doc.put(CATALOG, "AcroForm", V::Ref(acroform));
    if xfa_present {
        let packet = b"<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"></xdp:xdp>".to_vec();
        match shape {
            XfaShape::Array => {
                let stream = doc.add_stream(dict(vec![]), packet);
                doc.put(
                    acroform,
                    "XFA",
                    V::Arr(vec![V::Str(b"xdp:xdp".to_vec()), V::Ref(stream)]),
                );
            }
            XfaShape::Stream => {
                let stream = doc.add_stream(dict(vec![]), packet);
                doc.put(acroform, "XFA", V::Ref(stream));
            }
            XfaShape::Integer => doc.put(acroform, "XFA", V::Int(7)),
            XfaShape::NeedsRendering => doc.put(CATALOG, "NeedsRendering", V::Bool(true)),
            XfaShape::NeedsRenderingFalse => doc.put(CATALOG, "NeedsRendering", V::Bool(false)),
        }
    }
    doc
}

// ---------------------------------------------------------------------------------------------
// Rule 3, `[annotation-dependent-kept]`.
// ---------------------------------------------------------------------------------------------

/// The shapes by which something kept reaches an annotation (ADR 0031 rule 3, *Fixtures* row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationShape {
    /// One annotation listed in two pages' `/Annots`.
    ListedOnTwoPages,
    /// An appearance stream shared by an annotation on each of two pages.
    SharedAppearance,
    /// A link on one page whose `/Hide` action names an annotation on another.
    ActionNamesAnnotation,
    /// One indirect `/Annots` array shared by two pages (#240), holding no annotation, so that the
    /// array is the only thing kept.
    SharedAnnotsArray,
}

/// Every [`AnnotationShape`].
pub const ANNOTATION_SHAPES: [AnnotationShape; 4] = [
    AnnotationShape::ListedOnTwoPages,
    AnnotationShape::SharedAppearance,
    AnnotationShape::ActionNamesAnnotation,
    AnnotationShape::SharedAnnotsArray,
];

fn square(doc: &mut Doc, page: u32) -> u32 {
    doc.add(dict(vec![
        ("Type", name("Annot")),
        ("Subtype", name("Square")),
        (
            "Rect",
            V::Arr(vec![V::Int(20), V::Int(20), V::Int(80), V::Int(80)]),
        ),
        ("P", V::Ref(page)),
    ]))
}

fn appearance(doc: &mut Doc, resources: V) -> u32 {
    doc.add_stream(
        dict(vec![
            ("Type", name("XObject")),
            ("Subtype", name("Form")),
            (
                "BBox",
                V::Arr(vec![V::Int(0), V::Int(0), V::Int(60), V::Int(60)]),
            ),
            ("Resources", resources),
        ]),
        b"0 0 60 60 re f".to_vec(),
    )
}

/// Rule 3's fixture for `shape` and its twin, on three pages. Page N has an annotation of its own
/// that nothing else names, except in the shared-array fixture, where the shared array replaces it. The fixture builds `shape` between page N and page 1; the twin
/// builds the same shape between pages 1 and 2, unrelated to page N.
pub fn annotation(shape: AnnotationShape, between_page_n: bool) -> Doc {
    let mut doc = Doc::with_pages(3);
    if !(between_page_n && shape == AnnotationShape::SharedAnnotsArray) {
        let own = square(&mut doc, PAGE_N);
        doc.push_to(PAGE_N, "Annots", V::Ref(own));
    }
    let (a, b) = if between_page_n {
        (PAGE_N, doc.page(1))
    } else {
        (doc.page(1), doc.page(2))
    };
    match shape {
        AnnotationShape::ListedOnTwoPages => {
            let shared = square(&mut doc, a);
            doc.push_to(a, "Annots", V::Ref(shared));
            doc.push_to(b, "Annots", V::Ref(shared));
        }
        AnnotationShape::SharedAppearance => {
            let stream = appearance(&mut doc, dict(vec![]));
            for page in [a, b] {
                let annotation = square(&mut doc, page);
                doc.put(annotation, "AP", dict(vec![("N", V::Ref(stream))]));
                doc.push_to(page, "Annots", V::Ref(annotation));
            }
        }
        AnnotationShape::ActionNamesAnnotation => {
            let target = square(&mut doc, a);
            doc.push_to(a, "Annots", V::Ref(target));
            let link = doc.add(dict(vec![
                ("Type", name("Annot")),
                ("Subtype", name("Link")),
                (
                    "Rect",
                    V::Arr(vec![V::Int(0), V::Int(0), V::Int(10), V::Int(10)]),
                ),
                (
                    "A",
                    dict(vec![
                        ("S", name("Hide")),
                        ("T", V::Ref(target)),
                        ("H", V::Bool(true)),
                    ]),
                ),
            ]));
            doc.push_to(b, "Annots", V::Ref(link));
        }
        AnnotationShape::SharedAnnotsArray => {
            // The shared array holds no annotation, so nothing in it is "listed on two pages":
            // the array itself is the only member of A the output keeps. It is page a's whole
            // list, so in the fixture page N has no annotation of its own (none is built).
            let array = doc.add(V::Arr(Vec::new()));
            doc.put(a, "Annots", V::Ref(array));
            doc.put(b, "Annots", V::Ref(array));
        }
    }
    doc
}

/// Rule 3's font twin (does not fire): page N's annotation has an appearance whose `/Resources`
/// names a font that kept page 1 also uses. The font is rule 5's to disclose, not rule 3's.
pub fn annotation_font_twin() -> Doc {
    let mut doc = Doc::with_pages(3);
    let font = doc.add(dict(vec![
        ("Type", name("Font")),
        ("Subtype", name("Type1")),
        ("BaseFont", name("Helvetica")),
    ]));
    let fonts = || dict(vec![("Font", dict(vec![("F1", V::Ref(font))]))]);
    let stream = appearance(&mut doc, fonts());
    let annotation = square(&mut doc, PAGE_N);
    doc.put(annotation, "AP", dict(vec![("N", V::Ref(stream))]));
    doc.push_to(PAGE_N, "Annots", V::Ref(annotation));
    let kept = doc.page(1);
    doc.put(kept, "Resources", fonts());
    doc
}

// ---------------------------------------------------------------------------------------------
// Rule 4, `[metadata-thumbnail]`.
// ---------------------------------------------------------------------------------------------

/// XMP's basic schema namespace, which holds `Thumbnails` (XMP part 1, 8.4).
pub const XMP_NS: &str = "http://ns.adobe.com/xap/1.0/";

/// The character encodings an XMP packet may use (XMP part 1, 7.3.2: UTF-8, UTF-16 and UTF-32,
/// either byte order). Each is written without a leading mark; the packet header's own U+FEFF
/// is the byte-order marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// UTF-8, no byte-order mark.
    Utf8,
    /// UTF-16, big-endian.
    Utf16Be,
    /// UTF-16, little-endian.
    Utf16Le,
    /// UTF-32, big-endian.
    Utf32Be,
    /// UTF-32, little-endian.
    Utf32Le,
}

/// Every [`Encoding`].
pub const ENCODINGS: [Encoding; 5] = [
    Encoding::Utf8,
    Encoding::Utf16Be,
    Encoding::Utf16Le,
    Encoding::Utf32Be,
    Encoding::Utf32Le,
];

/// `text` in `encoding`. No byte-order mark is prepended: an XMP packet carries its own, U+FEFF in
/// the header's `begin` attribute (XMP part 1, 7.3.2), which [`packet`] writes.
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    let mut out = Vec::new();
    match encoding {
        Encoding::Utf8 => out.extend_from_slice(text.as_bytes()),
        Encoding::Utf16Be | Encoding::Utf16Le => {
            for unit in text.encode_utf16() {
                out.extend_from_slice(&if encoding == Encoding::Utf16Be {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
        }
        Encoding::Utf32Be | Encoding::Utf32Le => {
            for c in text.chars() {
                let unit = u32::from(c);
                out.extend_from_slice(&if encoding == Encoding::Utf32Be {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
        }
    }
    out
}

/// An XMP packet whose `rdf:Description` binds `prefix` (empty for the default namespace) to
/// `namespace`, and carries a `Thumbnails` property in it when `thumbnail` is set; otherwise a
/// `CreatorTool` property in the same namespace.
pub fn packet(prefix: &str, namespace: &str, thumbnail: bool) -> String {
    let (declaration, qualified) = if prefix.is_empty() {
        (format!("xmlns=\"{namespace}\""), String::new())
    } else {
        (
            format!("xmlns:{prefix}=\"{namespace}\""),
            format!("{prefix}:"),
        )
    };
    let property = if thumbnail {
        format!(
            "<{qualified}Thumbnails><rdf:Alt><rdf:li rdf:parseType=\"Resource\">\
             <xmpGImg:format>JPEG</xmpGImg:format><xmpGImg:width>8</xmpGImg:width>\
             <xmpGImg:height>8</xmpGImg:height><xmpGImg:image>/9j/4AAQSkZJRg==</xmpGImg:image>\
             </rdf:li></rdf:Alt></{qualified}Thumbnails>"
        )
    } else {
        format!("<{qualified}CreatorTool>burrow fixture</{qualified}CreatorTool>")
    };
    format!(
        "<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
         <rdf:Description rdf:about=\"\" {declaration} \
         xmlns:xmpGImg=\"http://ns.adobe.com/xap/1.0/g/img/\">\
         {property}\
         </rdf:Description></rdf:RDF></x:xmpmeta>\
         <?xpacket end=\"w\"?>"
    )
}

/// The ways rule 4's thumbnail is written, or its stream cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailShape {
    /// `xmp:Thumbnails`, the conventional prefix, in `encoding`.
    Plain(Encoding),
    /// The namespace bound to another prefix, `q:Thumbnails`.
    ReboundPrefix,
    /// The namespace as the default namespace, an unprefixed `Thumbnails`.
    DefaultNamespace,
    /// A `/FlateDecode` metadata stream whose bytes are not a zlib stream.
    Undecodable,
    /// A packet that is not well-formed XML: cut before `</rdf:Description>`, so its elements are
    /// never closed.
    Unreadable,
}

/// Every [`ThumbnailShape`] that fires: each encoding, then the others.
pub fn thumbnail_shapes() -> Vec<ThumbnailShape> {
    let mut shapes: Vec<_> = ENCODINGS
        .iter()
        .map(|&e| ThumbnailShape::Plain(e))
        .collect();
    shapes.extend([
        ThumbnailShape::ReboundPrefix,
        ThumbnailShape::DefaultNamespace,
        ThumbnailShape::Undecodable,
        ThumbnailShape::Unreadable,
    ]);
    shapes
}

/// Bytes whose zlib header fails its own check (`(CMF * 256 + FLG) % 31 != 0`): no inflater can
/// start on them.
pub const NOT_ZLIB: &[u8] = b"\x78\x00not a deflate stream";

/// `data` as a valid zlib stream (RFC 1950) of one stored deflate block (RFC 1951, 3.2.4): header
/// `78 01`, which passes the header check, then the block and the Adler-32 of `data`.
pub fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let len = u16::try_from(data.len()).expect("one stored block holds at most 65535 bytes");
    let mut out = vec![0x78, 0x01, 0x01];
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(!len).to_le_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Adler-32 (RFC 1950, 8.2).
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// A two-page document whose catalogue `/Metadata` stream holds `data`, with `filter` if given.
pub fn with_metadata(data: Vec<u8>, filter: Option<&str>) -> Doc {
    let mut doc = Doc::with_pages(2);
    let mut stream = vec![("Type", name("Metadata")), ("Subtype", name("XML"))];
    if let Some(filter) = filter {
        stream.push(("Filter", name(filter)));
    }
    let metadata = doc.add_stream(dict(stream), data);
    doc.put(CATALOG, "Metadata", V::Ref(metadata));
    doc
}

/// Rule 4's fixture for `shape`, or with `thumbnail` false, its twin: the same packet in the same
/// prefix, namespace and encoding, carrying `CreatorTool` instead of `Thumbnails`. The undecodable
/// shape's twin is the plain packet without a thumbnail under the same `/FlateDecode`, as a valid
/// zlib stream; the unreadable shape's, that packet whole.
pub fn metadata_thumbnail(shape: ThumbnailShape, thumbnail: bool) -> Doc {
    match shape {
        ThumbnailShape::Plain(encoding) => {
            with_metadata(encode(&packet("xmp", XMP_NS, thumbnail), encoding), None)
        }
        ThumbnailShape::ReboundPrefix => {
            with_metadata(packet("q", XMP_NS, thumbnail).into_bytes(), None)
        }
        ThumbnailShape::DefaultNamespace => {
            with_metadata(packet("", XMP_NS, thumbnail).into_bytes(), None)
        }
        ThumbnailShape::Undecodable if thumbnail => {
            with_metadata(NOT_ZLIB.to_vec(), Some("FlateDecode"))
        }
        ThumbnailShape::Unreadable if thumbnail => {
            let whole = packet("xmp", XMP_NS, false);
            let cut = whole.find("</rdf:Description>").unwrap();
            with_metadata(whole.as_bytes()[..cut].to_vec(), None)
        }
        ThumbnailShape::Undecodable => with_metadata(
            zlib_stored(packet("xmp", XMP_NS, false).as_bytes()),
            Some("FlateDecode"),
        ),
        ThumbnailShape::Unreadable => {
            with_metadata(packet("xmp", XMP_NS, false).into_bytes(), None)
        }
    }
}

/// Rule 4's other-namespace twin (does not fire): `xmp:Thumbnails`, the conventional prefix and
/// local name, bound to a namespace that is not XMP's.
pub fn thumbnail_in_another_namespace() -> Doc {
    with_metadata(
        packet("xmp", "http://example.com/not-xmp/1.0/", true).into_bytes(),
        None,
    )
}
