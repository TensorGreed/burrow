//! Every rule a redaction, a region or a page frame can refuse with, by the id its message
//! brackets (#136).
//!
//! # Why a table
//!
//! The ids are raised three ways: the geometry walk's [`Refusal`] names, stamped into the bracket
//! at run time; the frame rules' [`FrameRule::rule`]; and format strings written where a refusal
//! is raised (`"pdf redaction [font-flags]: …"`). No one of those lists the others, so nothing
//! outside this crate could ask "which rules are there" without parsing message shapes. The web
//! page groups refusals into plain-language categories and must place every one; it reads this
//! table, through the committed copy `tests/redaction/rule-ids.txt`, never the source.
//!
//! # Held to the source, both ways
//!
//! `every_bracketed_id_the_redaction_path_raises_is_in_the_table` reads every id the production
//! code of this crate and of `burrow-ops` brackets, plus the three name tables above and every rule
//! `tests/redaction/outcomes.tsv` records, and requires each in [`RULE_IDS`]; a rule added with any
//! name fails until it is listed. The reverse test refuses an entry nothing raises. A third holds
//! the committed copy to this table.
//!
//! Split's own rules (`split-*`) are split's and are not here.
//!
//! [`Refusal`]: crate::pdfsyntax::geometry::Refusal
//! [`FrameRule::rule`]: crate::redact::frames::FrameRule::rule

/// Every refusal rule id, sorted. See the module documentation for what keeps it complete.
pub const RULE_IDS: &[&str] = &[
    "acroform-field",
    "adjustment-not-expressible",
    "annotation-appearance-unbounded",
    "annotation-dependent-kept",
    "annotation-no-rotate",
    "annotation-popup-unreadable",
    "annotation-quads-outside-rect",
    "annotation-rect",
    "annots-too-long",
    "base-encoding-not-a-name",
    "bt-nested",
    "bt-unterminated",
    "contents-boundary",
    "contents-not-a-stream",
    "contents-offset",
    "contents-too-large",
    "contents-too-many",
    "contents-unreadable",
    "descendant-fonts",
    "differences-anchor",
    "differences-item-unreadable",
    "differences-too-long",
    "direct-font",
    "display-box",
    "do-in-text-object",
    "do-operand-not-a-name",
    "document-poisoned",
    "embedded-nul",
    "engine-repaired-input",
    "et-without-bt",
    "ext-gstate-line-unreadable",
    "ext-gstate-sets-font",
    "first-char",
    "font-flags",
    "font-missing",
    "font-selected-in-another-scope",
    "form-cycle",
    "form-depth",
    "form-graph-too-deep",
    "form-graph-too-large",
    "form-matrix",
    "form-unreadable",
    "form-vanished",
    "glyph-from-another-stream",
    "glyph-on-a-non-showing-operation",
    "glyph-without-its-operation",
    "graphics-state-in-text",
    "gs-operand-not-a-name",
    "high-byte-separator",
    "image-in-region",
    "inline-image-bpc-without-cs",
    "inline-image-cs-without-bpc",
    "inline-image-data-holds-ei",
    "inline-image-end-unrecognised",
    "inline-image-escaped-key",
    "inline-image-filtered",
    "inline-image-key-without-value",
    "inline-image-length-disagrees",
    "inline-image-mask-with-cs",
    "inline-image-nested-bi",
    "inline-image-nested-value",
    "inline-image-repeated-key",
    "inline-image-unknown-key",
    "inline-image-unshared-cs",
    "marked-content-carries-opaque-string",
    "marked-content-named-properties-carry-text",
    "marked-content-properties-unresolved",
    "marked-content-property-list-malformed",
    "marked-content-split-across-elements",
    "media-box-unverified",
    "missing-slash",
    "mixed-code-widths",
    "name-read-differently",
    "name-token-too-long",
    "no-display-box",
    "no-font-selected",
    "no-width",
    "no-widths",
    "non-finite-geometry",
    "not-a-dictionary-where-one-belongs",
    "null-value",
    "number-unreadable",
    "numeric-operand-not-a-number",
    "operand-count-mismatch",
    "optional-content",
    "optional-content-marked",
    "page-attribute-inherited",
    "page-frame-unreadable",
    "page-out-of-range",
    "page-tree-depth",
    "pattern-may-draw-text",
    "properties-too-many",
    "q-unbalanced",
    "q-without-save",
    "reference-to-nothing",
    "reference-unreadable",
    "region-extent",
    "resource-graph-cycle",
    "resource-graph-depth",
    "resource-graph-work",
    "rotate",
    "rotate-not-whole",
    "shading-in-region",
    "shared-contents",
    "shared-form-would-change-elsewhere",
    "show-operand-not-a-string",
    "simple-font-multi-byte-codes",
    "string-not-whole-codes",
    "subtype-not-a-name",
    "syntax-read-differently",
    "text-outside-text-object",
    "text-state-into-form",
    "tj-array-item-not-showable",
    "tj-operand-not-an-array",
    "too-many-form-draws",
    "too-many-glyphs",
    "too-much-ink",
    "type-three-image-cut",
    "type-three-image-outside-its-box",
    "type-three-procedure-matrix",
    "type-three-procedure-paints",
    "type-three-procedure-shows-text",
    "type-three-unreadable",
    "type-three-width-zero",
    "type3-matrix",
    "unknown-operator",
    "unreadable-cmap",
    "user-unit",
    "user-unit-not-one",
    "vector-in-region",
    "vertical-writing",
    "width-out-of-range",
    "width-source",
    "widths-overlap",
    "widths-too-long",
    "widths-too-many",
    "writing-mode-undetermined",
    "xobject-missing",
    "zero-bytes-per-code",
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use super::RULE_IDS;
    use crate::pdfsyntax::geometry::{InkKind, Refusal};
    use crate::redact::frames::FrameRule;

    /// The repository root, from this crate's manifest directory.
    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// Every `.rs` file under `dir`, recursively.
    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("a source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// A file that is test code by its name: `tests.rs`, `*_tests.rs`, or under `tests/`.
    fn is_test_file(path: &Path) -> bool {
        let text = path.to_string_lossy().replace('\\', "/");
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        name == "tests.rs"
            || name == "test.rs"
            || name.ends_with("_tests.rs")
            || text.contains("/tests/")
    }

    /// `text` with its inline `#[cfg(test)] mod … {` block, and everything after it, cut off.
    fn production(text: &str) -> &str {
        let mut from = 0;
        while let Some(at) = text[from..].find("#[cfg(test)]") {
            let start = from + at;
            let rest = text[start + "#[cfg(test)]".len()..].trim_start();
            let rest = rest
                .strip_prefix("pub(crate) ")
                .or_else(|| rest.strip_prefix("pub "))
                .unwrap_or(rest);
            if let Some(after) = rest.strip_prefix("mod ") {
                let head: String = after.chars().take_while(|c| *c != '\n').collect();
                if head.trim_end().ends_with('{') {
                    return &text[..start];
                }
            }
            from = start + 1;
        }
        text
    }

    fn is_id_byte(b: u8) -> bool {
        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
    }

    /// Every bracketed id in `code`: one with a hyphen anywhere, or one of any shape after the
    /// core's own prefixes, `pdf redaction [` and `pdf region [`.
    fn bracketed(code: &str, out: &mut BTreeSet<String>) {
        let bytes = code.as_bytes();
        let mut i = 0;
        while let Some(at) = code[i..].find('[') {
            let open = i + at;
            let mut end = open + 1;
            while end < bytes.len() && is_id_byte(bytes[end]) {
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b']' && end > open + 1 {
                let id = &code[open + 1..end];
                let prefixed = code[..open].ends_with("pdf redaction ")
                    || code[..open].ends_with("pdf region ");
                let shaped = id.starts_with(|c: char| c.is_ascii_lowercase())
                    && !id.ends_with('-')
                    && !id.contains("--");
                if shaped && (id.contains('-') || prefixed) {
                    out.insert(id.to_owned());
                }
            }
            i = open + 1;
        }
    }

    /// What the redaction path raises, read from the source and the name tables.
    fn raised() -> BTreeSet<String> {
        let mut files = Vec::new();
        rust_files(&repo().join("core/burrow-engines/src"), &mut files);
        rust_files(&repo().join("core/burrow-ops/src"), &mut files);
        let mut ids = BTreeSet::new();
        for path in files.iter().filter(|p| !is_test_file(p)) {
            let text = std::fs::read_to_string(path).expect("a source file");
            bracketed(production(&text), &mut ids);
        }
        ids.extend(Refusal::ALL.iter().map(|r| r.rule().to_owned()));
        ids.extend(FrameRule::ALL.iter().map(|r| r.rule().to_owned()));
        ids.extend(
            [InkKind::Image, InkKind::Vector, InkKind::Shading].map(|k| k.rule().to_owned()),
        );
        let outcomes = std::fs::read_to_string(repo().join("tests/redaction/outcomes.tsv"))
            .expect("the golden");
        bracketed(&outcomes, &mut ids);
        ids.retain(|id| !id.starts_with("split-"));
        ids
    }

    #[test]
    fn every_bracketed_id_the_redaction_path_raises_is_in_the_table() {
        let raised = raised();
        // A COUNT, so a scan that read nothing does not pass as "nothing missing".
        assert!(
            raised.len() > 100,
            "the scan found {} ids; it is not reading the source",
            raised.len()
        );
        let table: BTreeSet<&str> = RULE_IDS.iter().copied().collect();
        let missing: Vec<&String> = raised
            .iter()
            .filter(|id| !table.contains(id.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "rules raised but not in RULE_IDS: {missing:?}"
        );
    }

    #[test]
    fn the_table_lists_nothing_the_redaction_path_does_not_raise() {
        let raised = raised();
        let stale: Vec<&&str> = RULE_IDS
            .iter()
            .filter(|id| !raised.contains(**id))
            .collect();
        assert!(
            stale.is_empty(),
            "RULE_IDS entries nothing raises: {stale:?}"
        );
    }

    #[test]
    fn the_table_is_sorted_and_has_no_duplicates() {
        let mut sorted = RULE_IDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, RULE_IDS);
    }

    #[test]
    fn the_scan_reads_every_shape_and_leaves_test_code_out() {
        // THE SCAN'S OWN PROBE: one id per shape it must read, and the near-misses it must not.
        let mut found = BTreeSet::new();
        let code = concat!(
            "\"pdf redaction [planted-bracket]: x\"\n",
            "\"pdf region [plantedword]: a one-word rule after the prefix\"\n",
            "\"a word [notarule] with no hyphen and no prefix\"\n",
            "/// [`Refusal::Planted`] is a doc link, not a rule\n",
            "#[cfg(test)]\nmod hooks;\n",
            "\"after a test module declaration [still-production]\"\n",
            "#[cfg(test)]\nmod tests {\n",
            "    \"pdf redaction [only-in-tests]\"\n",
        );
        bracketed(production(code), &mut found);
        let found: Vec<&str> = found.iter().map(String::as_str).collect();
        assert_eq!(
            found,
            ["planted-bracket", "plantedword", "still-production"]
        );
        assert!(is_test_file(Path::new("a/src/b_tests.rs")));
        assert!(is_test_file(Path::new("a/src/tests/c.rs")));
        assert!(!is_test_file(Path::new("a/src/frames.rs")));
    }

    /// The committed copy the web page's test reads (`tests/redaction/rule-ids.txt`).
    fn committed() -> String {
        let mut text = String::from(
            "# Every redaction refusal rule id, one per line, written from\n\
             # burrow_engines::redact::rules::RULE_IDS by\n\
             #   BURROW_BLESS_RULE_IDS=1 cargo test -p burrow-engines --lib -- redact::rules\n\
             # and held to it on every run. apps/web's refusal categories must place every id.\n",
        );
        for id in RULE_IDS {
            text.push_str(id);
            text.push('\n');
        }
        text
    }

    #[test]
    fn the_committed_copy_is_the_table() {
        let path = repo().join("tests/redaction/rule-ids.txt");
        let expected = committed();
        if std::env::var_os("BURROW_BLESS_RULE_IDS").is_some() {
            std::fs::write(&path, &expected).expect("write the committed copy");
        }
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            on_disk, expected,
            "tests/redaction/rule-ids.txt is not RULE_IDS; re-bless with BURROW_BLESS_RULE_IDS=1"
        );
    }
}
