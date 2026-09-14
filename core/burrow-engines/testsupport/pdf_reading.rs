//! Reading a PDF burrow just produced, without a PDF parser.
//!
//! # Why this module exists, and what it cost not to have it
//!
//! Every operation's tests have to answer the same question — *what is actually in the bytes
//! we emitted?* — and every one of them has met the same two facts about qpdf's writer the
//! hard way:
//!
//! 1. **qpdf flates content streams on write.** A marker a fixture puts in a page's content
//!    stream is in the fixture and **not** in the output. `merge.rs` met it, `split.rs` wrote
//!    it down — *"a marker in a content stream is not in the output bytes at all, while page
//!    dictionaries are written plainly"* — and `reorder` met it again anyway.
//! 2. **qpdf spaces its arrays.** The fixture writes `[0 0 101 792]`; qpdf writes
//!    `[ 0 0 101 792 ]`. A reader that matches the fixture's spelling finds nothing, and one
//!    that counts characters reads the y-origin instead of the width. `split.rs` records that
//!    one too, one spelling further on.
//!
//! Both lessons were already written down, in prose, in a test file for a different operation.
//! They still cost `reorder` six failing tests against an implementation that was working —
//! which is the argument for a shared module rather than a shared comment. A lesson in prose
//! has to be read by the right person at the right moment; a function has to be imported.
//!
//! # What is here
//!
//! - [`expanded`] — the bytes with every stream decompressed, via the `qpdf` CLI. For anything
//!   that has to look *inside* a stream.
//! - [`page_order`] — the pages in page order, each identified by its `/MediaBox` width. For
//!   any operation that can change which page is where.
//! - [`numbers_in_array`] — the numbers in a PDF array, however it is spaced.
//! - [`object_body`] — one object's dictionary, by number.
//!
//! Each tolerates both spellings, so a caller cannot be caught by the difference between what
//! a fixture writes and what qpdf writes.

#![allow(dead_code, clippy::expect_used, clippy::panic)]

use std::process::{Command, Stdio};

/// The document with every stream decompressed.
///
/// **Shelling out to `qpdf`, and failing loudly if it is not there.** A scan that cannot
/// decompress reports silence rather than a finding — which is how a leak test came to pass
/// against output it could not read (ADR 0019's third correction). CI installs the CLI in the
/// `test` job and asserts it is on `PATH` before the suite runs.
///
/// Through temporary files rather than stdin: this qpdf refuses the `- -` invocation, and the
/// version that fell back to the raw bytes on failure reported compressed markers as absent.
/// There is no fallback here on purpose.
///
/// The temporary names carry the process **and the thread**, because the suite runs in
/// parallel and two tests expanding at once through one filename would each read the other's
/// document.
#[must_use]
pub fn expanded(bytes: &[u8]) -> Vec<u8> {
    let dir = std::env::temp_dir();
    let tag = format!(
        "burrow-expand-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    );
    let input = dir.join(format!("{tag}-in.pdf"));
    let output = dir.join(format!("{tag}-out.pdf"));
    std::fs::write(&input, bytes).expect("write the document to expand");

    let status = Command::new("qpdf")
        .args(["--qdf", "--object-streams=disable"])
        .arg(&input)
        .arg(&output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect(
            "the `qpdf` CLI is needed to decompress output before scanning it; without it a \
             marker inside a flated stream is invisible and the scan reports silence for the \
             wrong reason",
        );
    // 0 is clean, 3 is warnings -- both wrote a file.
    assert!(
        matches!(status.code(), Some(0 | 3)),
        "qpdf could not expand the document ({status:?}), so nothing could be scanned"
    );
    let expanded = std::fs::read(&output).expect("read the expanded document");
    assert!(
        !expanded.is_empty(),
        "qpdf produced an empty expansion, so nothing could be scanned"
    );
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&output);
    expanded
}

/// The numbers in the array that follows `key`, however the array is spaced.
///
/// `/MediaBox [0 0 101 792]` and `/MediaBox [ 0 0 101 792 ]` give the same answer, which is the
/// whole point: the first is what a fixture writes and the second is what qpdf writes.
///
/// Returns an empty vector if the key or its array is absent, so a caller that gets nothing
/// knows the shape was not there rather than being told a wrong number.
#[must_use]
pub fn numbers_in_array(text: &str, key: &str) -> Vec<u64> {
    let Some(at) = text.find(key) else {
        return Vec::new();
    };
    let after = &text[at..];
    let (Some(open), Some(close)) = (after.find('['), after.find(']')) else {
        return Vec::new();
    };
    after[open + 1..close]
        .split_whitespace()
        .filter_map(|token| token.parse::<u64>().ok())
        .collect()
}

/// The text of object `number`'s body, from `N 0 obj` to `endobj`.
///
/// `None` if the object is not there, and **never text belonging to another object**.
///
/// # Two ways to get this wrong, one of which was shipped and one of which was nearly shipped
///
/// The two readers this replaced wrote `body.find("endobj")?`. The merged version relaxed that
/// to `unwrap_or(body.len())`, so an object with no `endobj` ran to EOF and picked up the
/// **next** object's `/MediaBox` — a plausible wrong width rather than a short vector, which
/// is the outcome this module's rustdoc says it refuses to give. Found by code review.
///
/// Restoring the `?` was not enough, and the regression test written for it is what showed
/// that: a *later* object's `endobj` terminates the scan just as happily, so the body still
/// spanned two objects. The scan stops at whichever comes first — this object's `endobj`, or
/// the header of the next object — which is stricter than either earlier version.
#[must_use]
pub fn object_body(text: &str, number: u64) -> Option<&str> {
    let at = text.find(&format!("\n{number} 0 obj"))?;
    let body = &text[at..];
    // Past this object's own header, so `find` cannot match it.
    let after_header = body.find(" 0 obj")? + " 0 obj".len();
    let next_object = body[after_header..]
        .find(" 0 obj")
        .map(|n| n + after_header);
    let end = match (body.find("endobj"), next_object) {
        (Some(endobj), Some(next)) if next < endobj => return None,
        (Some(endobj), _) => endobj,
        // No `endobj` at all: the object is malformed or truncated, and a body without a
        // terminator is not a body.
        (None, _) => return None,
    };
    Some(&body[..end])
}

/// Each page's `/MediaBox` width, in **page-tree order**.
///
/// # Why the width, and why the tree
///
/// A fixture whose pages are interchangeable cannot fail an order test — the `add-operation`
/// checklist's §2c records that mistake twice over — so every fixture here gives each page a
/// distinct width and a width says which page it is. `merge`, `split` and `reorder` all do
/// this, independently, for the same reason.
///
/// The order is the **page tree's**, not the order the objects happen to be written in.
/// Reading them in file order would pass an operation that wrote the pages out of sequence
/// but listed them correctly, and fail one that did the reverse; neither is the question.
///
/// # It walks, because the tree is not always flat
///
/// Reading the first `/Kids` array is right for a flattened tree and wrong for a nested one.
/// `reorder` produces both: qpdf flattens when a page moves, and the identity permutation
/// moves nothing, so its output keeps the structure it arrived with (ADR 0021). The reader
/// that assumed one shape reported the root's branch nodes as pages.
///
/// Returns an empty vector if the page tree is not written plainly, so a test fails against a
/// full expected sequence rather than passing vacuously.
#[must_use]
pub fn page_widths(bytes: &[u8]) -> Vec<u64> {
    let text = String::from_utf8_lossy(bytes);
    let Some(root) = root_pages_object(&text) else {
        return Vec::new();
    };
    let mut widths = Vec::new();
    walk_pages(&text, root, &mut widths, 0);
    widths
}

/// The same, as **one-based page numbers**, for fixtures using the `101 + i` convention.
///
/// `pdf_with_page_tree` gives page `i` the width `101 + i`; widths start at 101 so none can be
/// confused with a count, a generation number, or the `0 0` of a `/MediaBox` origin. A width
/// this reader cannot turn into a page number is dropped, which shortens the vector and fails
/// the comparison rather than inventing a page.
#[must_use]
pub fn page_order(bytes: &[u8]) -> Vec<u64> {
    page_widths(bytes)
        .into_iter()
        .filter_map(|width| width.checked_sub(100))
        .collect()
}

/// The object number of the catalog's `/Pages`.
///
/// # The first `/Pages` that is actually a REFERENCE
///
/// Taking the first `/Pages` in the file outright assumes no `/Type /Pages` node is written
/// before the catalog: if one were, the token after it would be `/Count` or `/Kids`, the parse
/// would fail, and every caller would get an empty vector. Loud rather than vacuous, which is
/// the right direction — but it made the reader depend on an object ordering nobody promised.
/// Found by code review.
///
/// **Anchoring on `/Type /Catalog` and searching forward from there is the obvious fix and it
/// is wrong.** qpdf writes dictionary keys sorted, so a catalog reads
/// `<< /Pages 2 0 R /Type /Catalog >>` — `/Pages` comes *before* `/Type`, and searching after
/// the anchor skips the catalog's own key and finds the page tree's `/Type /Pages` instead.
/// Every reorder test went red on that, measured against real qpdf output.
///
/// So the shape is the discriminator rather than the position: `/Pages` followed by `N 0 R` is
/// a reference to a page tree; `/Pages` followed by anything else is the tail of `/Type
/// /Pages` and is skipped.
fn root_pages_object(text: &str) -> Option<u64> {
    let mut from = 0;
    while let Some(offset) = text[from..].find("/Pages") {
        let at = from + offset + "/Pages".len();
        let tokens: Vec<&str> = text[at..].split_whitespace().take(3).collect();
        if let (Some(number), Some(&"R")) = (
            tokens.first().and_then(|t| t.parse::<u64>().ok()),
            tokens.get(2),
        ) {
            return Some(number);
        }
        from = at;
    }
    None
}

/// Collect the widths of the pages under `object`, in order.
fn walk_pages(text: &str, object: u64, out: &mut Vec<u64>, depth: usize) {
    // A `/Kids` cycle in a document these tests generated would hang the suite rather than fail
    // it, and no fixture here is deeper than a handful of levels.
    if depth > 8 {
        return;
    }
    let Some(body) = object_body(text, object) else {
        return;
    };

    if body.contains("/Kids") {
        // `[3 0 R 4 0 R ...]` -- the object number is the first of each pair.
        for child in numbers_in_array(body, "/Kids")
            .chunks(2)
            .filter_map(|pair| pair.first().copied())
        {
            walk_pages(text, child, out, depth + 1);
        }
        return;
    }

    // A leaf: a page. A `/MediaBox` IS FOUR NUMBERS -- anything else means this reader is
    // looking at something it does not understand, and pushing nothing makes the caller fail
    // with a short vector rather than a plausible wrong one.
    let media_box = numbers_in_array(body, "/MediaBox");
    if let (4, Some(width)) = (media_box.len(), media_box.get(2)) {
        out.push(*width);
    }
}

#[cfg(test)]
mod tests {
    use super::{numbers_in_array, object_body, page_order, page_widths};

    #[test]
    fn an_array_reads_the_same_however_it_is_spaced() {
        // THE LESSON THIS MODULE EXISTS FOR, asserted rather than described. The first is what
        // a fixture writes and the second is what qpdf writes.
        assert_eq!(
            numbers_in_array("/MediaBox [0 0 101 792]", "/MediaBox"),
            vec![0, 0, 101, 792]
        );
        assert_eq!(
            numbers_in_array("/MediaBox [ 0 0 101 792 ]", "/MediaBox"),
            vec![0, 0, 101, 792]
        );
        // And a newline-separated one, which `--qdf` produces.
        assert_eq!(
            numbers_in_array("/MediaBox [\n  0\n  0\n  101\n  792\n]", "/MediaBox"),
            vec![0, 0, 101, 792]
        );
    }

    #[test]
    fn an_absent_array_reads_as_nothing_rather_than_as_a_number() {
        assert!(numbers_in_array("<< /Type /Page >>", "/MediaBox").is_empty());
        assert!(numbers_in_array("/MediaBox", "/MediaBox").is_empty());
    }

    #[test]
    fn an_object_with_no_endobj_reads_as_nothing_rather_than_as_the_rest_of_the_file() {
        // THE REGRESSION TEST FOR THE PROPERTY LOST IN THE MOVE. Without the `?`, object 3's
        // body ran to EOF and picked up object 4's `/MediaBox` -- reporting 102 as page 3's
        // width, a plausible wrong answer rather than a short vector.
        let text = "\n3 0 obj\n<< /Type /Page /MediaBox [ 0 0 101 792 ] >>\n\
                    4 0 obj\n<< /Type /Page /MediaBox [ 0 0 102 792 ] >>\nendobj\n";
        assert!(object_body(text, 3).is_none());
    }

    #[test]
    fn a_pages_node_written_before_the_catalog_is_not_mistaken_for_the_root() {
        // The first `/Pages` in this file is the tail of `/Type /Pages`, followed by `/Kids`.
        // A reader that took it would parse nothing and report an empty document.
        let text = "\n9 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n\
                    1 0 obj\n<< /Pages 9 0 R /Type /Catalog >>\nendobj\n\
                    3 0 obj\n<< /Type /Page /MediaBox [ 0 0 101 792 ] >>\nendobj\n";
        assert_eq!(page_order(text.as_bytes()), vec![1]);
    }

    #[test]
    fn the_catalog_is_found_however_its_keys_are_ordered() {
        // qpdf writes keys SORTED, so `/Pages` precedes `/Type` in a real catalog. A reader
        // anchored on `/Type /Catalog` and searching forward skips the key it is looking for
        // -- measured against real output, where it turned every reorder test red.
        for catalog in [
            "<< /Pages 2 0 R /Type /Catalog >>",
            "<< /Type /Catalog /Pages 2 0 R >>",
        ] {
            let text = format!(
                "\n1 0 obj\n{catalog}\nendobj\n\
                 2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n\
                 3 0 obj\n<< /Type /Page /MediaBox [ 0 0 101 792 ] >>\nendobj\n"
            );
            assert_eq!(page_order(text.as_bytes()), vec![1], "catalog: {catalog}");
        }
    }

    #[test]
    fn an_object_body_stops_at_endobj() {
        let text = "\n3 0 obj\n<< /A 1 >>\nendobj\n4 0 obj\n<< /B 2 >>\nendobj\n";
        let body = object_body(text, 3).expect("object 3");
        assert!(body.contains("/A 1"));
        assert!(
            !body.contains("/B 2"),
            "it ran into the next object: {body}"
        );
    }

    #[test]
    fn a_flat_page_tree_reads_in_order() {
        let text = "\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] >>\nendobj\n\
                    3 0 obj\n<< /Type /Page /MediaBox [ 0 0 101 792 ] >>\nendobj\n\
                    4 0 obj\n<< /Type /Page /MediaBox [ 0 0 102 792 ] >>\nendobj\n";
        assert_eq!(page_order(text.as_bytes()), vec![1, 2]);
    }

    #[test]
    fn a_nested_page_tree_reads_in_order_too() {
        // The shape `reorder`'s identity permutation leaves behind, and the one a reader that
        // took the first `/Kids` array got wrong -- it reported the branch nodes as pages.
        let text = "\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 6 0 R ] >>\nendobj\n\
                    3 0 obj\n<< /Type /Pages /Kids [ 4 0 R 5 0 R ] >>\nendobj\n\
                    4 0 obj\n<< /Type /Page /MediaBox [ 0 0 101 792 ] >>\nendobj\n\
                    5 0 obj\n<< /Type /Page /MediaBox [ 0 0 102 792 ] >>\nendobj\n\
                    6 0 obj\n<< /Type /Page /MediaBox [ 0 0 103 792 ] >>\nendobj\n";
        assert_eq!(page_order(text.as_bytes()), vec![1, 2, 3]);
    }

    #[test]
    fn a_reordered_tree_reads_in_the_new_order() {
        // The assertion every reorder test rests on: the order is `/Kids`, not the order the
        // objects happen to appear in the file.
        let text = "\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages /Kids [ 4 0 R 3 0 R ] >>\nendobj\n\
                    3 0 obj\n<< /Type /Page /MediaBox [ 0 0 101 792 ] >>\nendobj\n\
                    4 0 obj\n<< /Type /Page /MediaBox [ 0 0 102 792 ] >>\nendobj\n";
        assert_eq!(page_order(text.as_bytes()), vec![2, 1]);
    }

    #[test]
    fn a_cycle_is_refused_rather_than_followed_forever() {
        // A generated fixture cannot do this, and a hostile document could -- and a test suite
        // that hangs is worse than one that fails.
        let text = "\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages /Kids [ 2 0 R ] >>\nendobj\n";
        assert!(page_order(text.as_bytes()).is_empty());
    }

    #[test]
    fn the_raw_widths_are_available_too() {
        // `merge` and `split` give their pages arbitrary widths rather than the `101 + i`
        // convention, so they read widths directly. Both readers are the same walk.
        let text = "\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] >>\nendobj\n\
                    3 0 obj\n<< /Type /Page /MediaBox [ 0 0 300 200 ] >>\nendobj\n\
                    4 0 obj\n<< /Type /Page /MediaBox [ 0 0 200 200 ] >>\nendobj\n";
        assert_eq!(page_widths(text.as_bytes()), vec![300, 200]);
    }

    #[test]
    fn a_media_box_that_is_not_four_numbers_is_dropped_rather_than_guessed_at() {
        // A short vector fails the comparison. A plausible wrong number would pass one.
        let text = "\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
                    2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] >>\nendobj\n\
                    3 0 obj\n<< /Type /Page /MediaBox [ 0 0 101 ] >>\nendobj\n\
                    4 0 obj\n<< /Type /Page /MediaBox [ 0 0 102 792 ] >>\nendobj\n";
        assert_eq!(page_widths(text.as_bytes()), vec![102]);
    }
}
