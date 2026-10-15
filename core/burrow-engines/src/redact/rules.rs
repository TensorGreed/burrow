//! Every rule a redaction, a region or a page frame can refuse with, by the id its message
//! brackets (#136).
//!
//! # Why a table
//!
//! The ids are raised three ways: the geometry walk's [`Refusal`] names, stamped into the bracket
//! at run time; the frame rules' [`FrameRule::rule`] and the ink kinds' [`InkKind::rule`]; and
//! literal messages written where a refusal is raised (`"pdf redaction [font-flags]: …"`). No one
//! of those lists the others, so nothing outside this crate could ask "which rules are there"
//! without parsing message shapes. The web page groups refusals into plain-language categories and
//! must place every one; it reads this table, through the committed copy
//! `tests/redaction/rule-ids.txt`, never the source.
//!
//! # What the tests guarantee, exactly
//!
//! An INTERIM, until the ids are one enum every refusal is built from (filed, post-launch). The
//! tests below read the production source of this crate and of `burrow-ops` -- comments, test
//! files, file modules declared under `#[cfg(test)]` and inline `#[cfg(test)] mod … { … }`
//! blocks left out, and code after such a block still read -- and require:
//!
//! - every id bracketed after a message prefix in a string literal (`"<lowercase words> [` --
//!   `pdf redaction [`, `pdf region [`, `pdf syntax [`, …), of any shape, is in [`RULE_IDS`];
//! - every hyphenated `[id]` in any production string literal is in it too;
//! - no production string literal interpolates into a bracket (`[{`), outside an explicit list of
//!   three whole literals, each in its own file and each met exactly once: the two that stamp a
//!   table's name ([`InkKind`]'s, and [`Refusal`]'s), and one that only READS a refusal's prefix
//!   to match it and raises nothing;
//! - every name in [`Refusal::ALL`], [`FrameRule::ALL`] and [`InkKind::ALL`] is in it, each `ALL`
//!   held to its enum by an exhaustive match or its own test, and so is every rule
//!   `tests/redaction/outcomes.tsv` records;
//! - nothing is in [`RULE_IDS`] that none of those raise;
//! - and the committed copy is the table.
//!
//! What it does not see, until #360's enum: an id assembled any other way. Three named shapes --
//! a literal built with `concat!`; a message whose prefix is itself interpolated
//! (`"{PREFIX} [word]"`, which is not "lowercase words, then `[`", so a hyphenless id there is not
//! read); and a bracketed id passed as a format ARGUMENT (`format!("pdf redaction {}: …",
//! "[word]")`), whose literal holds no bracket to refuse. The interpolation rule closes the common
//! form; the enum closes them all.
//!
//! Split's own rules (`split-*`) are split's and are not here.
//!
//! [`Refusal`]: crate::pdfsyntax::geometry::Refusal
//! [`Refusal::ALL`]: crate::pdfsyntax::geometry::Refusal::ALL
//! [`InkKind`]: crate::pdfsyntax::geometry::InkKind
//! [`InkKind::rule`]: crate::pdfsyntax::geometry::InkKind::rule
//! [`InkKind::ALL`]: crate::pdfsyntax::geometry::InkKind::ALL
//! [`FrameRule::rule`]: crate::redact::frames::FrameRule::rule
//! [`FrameRule::ALL`]: crate::redact::frames::FrameRule::ALL

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

    /// A Rust source, lexed just enough: `masked` is the text with every comment, string and
    /// character literal blanked (newlines kept), and `strings` every string literal's start
    /// offset and contents. Raw strings, byte strings, nested block comments, escapes and
    /// lifetimes are handled; that is all a brace count and a string scan need.
    struct Lexed {
        masked: Vec<u8>,
        strings: Vec<(usize, String)>,
    }

    fn lex(text: &str) -> Lexed {
        let b = text.as_bytes();
        let mut masked = b.to_vec();
        let mut strings = Vec::new();
        let blank = |m: &mut Vec<u8>, from: usize, to: usize| {
            for x in &mut m[from..to] {
                if *x != b'\n' {
                    *x = b' ';
                }
            }
        };
        let mut i = 0;
        while i < b.len() {
            if b[i..].starts_with(b"//") {
                let end = text[i..].find('\n').map_or(b.len(), |n| i + n);
                blank(&mut masked, i, end);
                i = end;
            } else if b[i..].starts_with(b"/*") {
                let (mut depth, mut j) = (1, i + 2);
                while j < b.len() && depth > 0 {
                    if b[j..].starts_with(b"/*") {
                        depth += 1;
                        j += 2;
                    } else if b[j..].starts_with(b"*/") {
                        depth -= 1;
                        j += 2;
                    } else {
                        j += 1;
                    }
                }
                blank(&mut masked, i, j);
                i = j;
            } else if (b[i] == b'r' || b[i..].starts_with(b"br"))
                && !(i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
                && {
                    let k = if b[i] == b'b' { i + 2 } else { i + 1 };
                    let mut h = k;
                    while h < b.len() && b[h] == b'#' {
                        h += 1;
                    }
                    h < b.len() && b[h] == b'"'
                }
            {
                let k = if b[i] == b'b' { i + 2 } else { i + 1 };
                let mut h = k;
                while b[h] == b'#' {
                    h += 1;
                }
                let hashes = h - k;
                let close: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                let body = h + 1;
                let end = text[body..].find(&close).map_or(b.len(), |n| body + n);
                strings.push((i, text[body..end].to_owned()));
                let stop = (end + close.len()).min(b.len());
                blank(&mut masked, i, stop);
                i = stop;
            } else if b[i] == b'"' {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                let end = j.min(b.len());
                strings.push((i, text[i + 1..end].to_owned()));
                let stop = (end + 1).min(b.len());
                blank(&mut masked, i, stop);
                i = stop;
            } else if b[i] == b'\'' {
                // A character literal, or a lifetime: `'x'` and `'\n'` close; `'a` does not.
                if i + 1 < b.len() && b[i + 1] == b'\\' {
                    let end = text[i + 2..].find('\'').map_or(b.len(), |n| i + 2 + n + 1);
                    blank(&mut masked, i, end);
                    i = end;
                } else if let Some(c) = text[i + 1..].chars().next() {
                    let after = i + 1 + c.len_utf8();
                    if after < b.len() && b[after] == b'\'' {
                        blank(&mut masked, i, after + 1);
                        i = after + 1;
                    } else {
                        i += 1;
                    }
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        Lexed { masked, strings }
    }

    /// Whether a `cfg` predicate holds only under test: `test`, or an `all(…)` with `test` among
    /// its terms. `not(test)`, `any(…)` and everything else is production.
    fn requires_test(predicate: &str) -> bool {
        let predicate = predicate.trim();
        if predicate == "test" {
            return true;
        }
        predicate
            .strip_prefix("all(")
            .and_then(|r| r.strip_suffix(')'))
            .is_some_and(|terms| terms.split(',').any(|t| t.trim() == "test"))
    }

    /// The byte spans of every inline test-only `mod NAME { … }` block (`#[cfg(test)]`, or a
    /// `cfg(all(test, …))`), matched by braces in the masked text, and the names of file modules
    /// declared test-only (`mod NAME;`).
    fn test_modules(lexed: &Lexed) -> (Vec<(usize, usize)>, Vec<String>) {
        let code = String::from_utf8_lossy(&lexed.masked).into_owned();
        let (mut spans, mut files) = (Vec::new(), Vec::new());
        let mut from = 0;
        while let Some(at) = code[from..].find("#[cfg(") {
            let start = from + at;
            let body = &code[start + "#[cfg(".len()..];
            let Some(close) = body.find(")]") else { break };
            if !requires_test(&body[..close]) {
                from = start + 1;
                continue;
            }
            let mut rest = body[close + 2..].trim_start();
            while rest.starts_with("#[") {
                rest = rest[rest.find(']').map_or(rest.len(), |n| n + 1)..].trim_start();
            }
            for vis in ["pub(crate) ", "pub(super) ", "pub "] {
                if let Some(r) = rest.strip_prefix(vis) {
                    rest = r;
                }
            }
            if let Some(after) = rest.strip_prefix("mod ") {
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                let tail = after[name.len()..].trim_start();
                if tail.starts_with(';') {
                    files.push(name);
                } else if tail.starts_with('{') {
                    let open = code.len() - tail.len();
                    let mut depth = 0usize;
                    let mut end = code.len();
                    for (k, c) in code[open..].char_indices() {
                        match c {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = open + k + 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    spans.push((start, end));
                    from = end;
                    continue;
                }
            }
            from = start + 1;
        }
        (spans, files)
    }

    /// The string literals of production code: everything but inline test modules.
    fn production_strings(text: &str) -> (Vec<String>, Vec<String>) {
        let lexed = lex(text);
        let (spans, files) = test_modules(&lexed);
        let strings = lexed
            .strings
            .iter()
            .filter(|(at, _)| !spans.iter().any(|(s, e)| at >= s && at < e))
            .map(|(_, s)| s.clone())
            .collect();
        (strings, files)
    }

    /// Whether `literal` starts with a message prefix: lowercase words, a space, then `[`.
    fn prefixed_id(literal: &str) -> Option<&str> {
        let open = literal.find(" [")?;
        let head = &literal[..open];
        if head.is_empty()
            || !head
                .split(' ')
                .all(|w| !w.is_empty() && w.bytes().all(|c| c.is_ascii_lowercase()))
        {
            return None;
        }
        let body = &literal[open + 2..];
        Some(&body[..body.find(']')?])
    }

    /// Every hyphenated `[id]` in a string literal.
    fn hyphenated(literal: &str, out: &mut BTreeSet<String>) {
        let mut rest = literal;
        while let Some(open) = rest.find('[') {
            let body = &rest[open + 1..];
            if let Some(close) = body.find(']') {
                let id = &body[..close];
                if id.contains('-')
                    && id.starts_with(|c: char| c.is_ascii_lowercase())
                    && id
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                {
                    out.insert(id.to_owned());
                }
            }
            rest = body;
        }
    }

    /// The interpolations into a bracket that may stand: each a WHOLE literal, in one file, that
    /// stamps a table's name. Compared with `==` after Rust's line continuations are folded, and
    /// each must be met exactly once, so a new literal with a stamped prefix is not admitted.
    const STAMPED: &[(&str, &str)] = &[
        // `InkKind::rule`, in the region-reaches-ink refusal.
        (
            "redact/steps.rs",
            "pdf redaction [{}]: the redacted region reaches {}, which the redaction removes no part of, so the region cannot be cleared",
        ),
        // `Refusal::rule`, where `Refusal::refuse` builds the walk's every refusal.
        ("pdfsyntax/geometry.rs", "pdf geometry [{}]: {detail}"),
        // `Refusal::caught`, which only READS a refusal's prefix, to match one; it raises nothing.
        ("pdfsyntax/geometry.rs", "pdf geometry [{}]: "),
    ];

    /// A string literal's contents with Rust's line continuations (`\` at a line's end, and the
    /// next line's leading whitespace) folded away, as the compiler folds them.
    fn folded(literal: &str) -> String {
        let mut out = String::new();
        let mut rest = literal;
        while let Some(at) = rest.find("\\\n") {
            out.push_str(&rest[..at]);
            rest = rest[at + 2..].trim_start();
        }
        out.push_str(rest);
        out
    }

    /// What one source raises; the interpolations into a bracket it makes that `STAMPED` does not
    /// name; and, per `STAMPED` entry, how many times it was met (`stamped`).
    fn scan(
        path: &str,
        text: &str,
        ids: &mut BTreeSet<String>,
        interpolated: &mut Vec<String>,
        stamped: &mut [usize],
    ) -> Vec<String> {
        let (strings, files) = production_strings(text);
        for literal in &strings {
            if let Some(id) = prefixed_id(literal)
                && !id.contains('{')
            {
                ids.insert(id.to_owned());
            }
            hyphenated(literal, ids);
            if literal.contains("[{") {
                let whole = folded(literal);
                match STAMPED
                    .iter()
                    .position(|(file, site)| path.ends_with(file) && whole == *site)
                {
                    Some(entry) => stamped[entry] += 1,
                    None => interpolated.push(format!("{path}: {whole}")),
                }
            }
        }
        files
    }

    struct Raised {
        ids: BTreeSet<String>,
        interpolated: Vec<String>,
        /// How many times each `STAMPED` entry was met, which must be once each.
        stamped: Vec<usize>,
    }

    fn raised() -> Raised {
        let mut files = Vec::new();
        rust_files(&repo().join("core/burrow-engines/src"), &mut files);
        rust_files(&repo().join("core/burrow-ops/src"), &mut files);
        // FILE MODULES DECLARED UNDER `#[cfg(test)]`, which are test code whatever they are called.
        let mut test_paths: BTreeSet<PathBuf> = BTreeSet::new();
        for path in &files {
            let text = std::fs::read_to_string(path).expect("a source file");
            let (_, names) = test_modules(&lex(&text));
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            let base = if stem == "mod" || stem == "lib" {
                dir
            } else {
                dir.join(&stem)
            };
            for name in names {
                test_paths.insert(base.join(format!("{name}.rs")));
                test_paths.insert(base.join(&name).join("mod.rs"));
            }
        }
        let mut ids = BTreeSet::new();
        let mut interpolated = Vec::new();
        let mut stamped = vec![0; STAMPED.len()];
        for path in files
            .iter()
            .filter(|p| !is_test_file(p) && !test_paths.contains(*p))
        {
            let text = std::fs::read_to_string(path).expect("a source file");
            let shown = path.to_string_lossy().replace('\\', "/");
            scan(&shown, &text, &mut ids, &mut interpolated, &mut stamped);
        }
        ids.extend(Refusal::ALL.iter().map(|r| r.rule().to_owned()));
        ids.extend(FrameRule::ALL.iter().map(|r| r.rule().to_owned()));
        ids.extend(InkKind::ALL.iter().map(|k| k.rule().to_owned()));
        let outcomes = std::fs::read_to_string(repo().join("tests/redaction/outcomes.tsv"))
            .expect("the golden");
        for line in outcomes.lines().filter(|l| l.starts_with("CASE")) {
            hyphenated(line, &mut ids);
            if let Some(at) = line.find("(\"")
                && let Some(id) = prefixed_id(&line[at + 2..])
            {
                ids.insert(id.to_owned());
            }
        }
        ids.retain(|id| !id.starts_with("split-"));
        Raised {
            ids,
            interpolated,
            stamped,
        }
    }

    #[test]
    fn every_bracketed_id_the_redaction_path_raises_is_in_the_table() {
        let raised = raised();
        // A COUNT, so a scan that read nothing does not pass as "nothing missing".
        assert!(
            raised.ids.len() > 100,
            "the scan found {} ids; it is not reading the source",
            raised.ids.len()
        );
        let table: BTreeSet<&str> = RULE_IDS.iter().copied().collect();
        let missing: Vec<&String> = raised
            .ids
            .iter()
            .filter(|id| !table.contains(id.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "rules raised but not in RULE_IDS: {missing:?}"
        );
    }

    #[test]
    fn no_production_message_builds_a_bracketed_id_the_scan_cannot_read() {
        let raised = raised();
        assert!(
            raised.interpolated.is_empty(),
            "an id interpolated into a bracket outside the stamped sites: {:?}",
            raised.interpolated
        );
        // REPORTED, AND EACH ONCE: an entry met twice would admit a second literal, and one met
        // never names a site that no longer exists.
        let met: Vec<(&str, usize)> = STAMPED
            .iter()
            .zip(&raised.stamped)
            .map(|((file, _), n)| (*file, *n))
            .collect();
        assert!(
            raised.stamped.iter().all(|n| *n == 1),
            "each stamped site must be met exactly once; met: {met:?}"
        );
    }

    #[test]
    fn the_table_lists_nothing_the_redaction_path_does_not_raise() {
        let raised = raised();
        let stale: Vec<&&str> = RULE_IDS
            .iter()
            .filter(|id| !raised.ids.contains(**id))
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
    fn ink_kind_all_is_every_variant() {
        // EXHAUSTIVE: a new variant does not compile until it has an index here, and the index
        // count is then held against `ALL`.
        fn index(kind: InkKind) -> usize {
            match kind {
                InkKind::Image => 0,
                InkKind::Vector => 1,
                InkKind::Shading => 2,
            }
        }
        let seen: BTreeSet<usize> = InkKind::ALL.iter().map(|k| index(*k)).collect();
        assert_eq!(seen, (0..3).collect::<BTreeSet<_>>());
        assert_eq!(InkKind::ALL.len(), 3);
    }

    #[test]
    fn the_scan_reads_every_shape_and_leaves_test_code_out() {
        // THE SCAN'S OWN PROBE, PERMANENT: each escape round 2 planted, and the near-misses.
        let source = concat!(
            "fn a() -> Error { Error::Malformed(\"pdf syntax [plantedword]: x\".to_owned()) }\n",
            "const PLANTED: &str = \"planted-const\";\n",
            "fn b() -> String { format!(\"pdf redaction [{PLANTED}]: x\") }\n",
            "fn c() -> &'static str { \"pdf redaction [planted-bracket]: x\" }\n",
            "// pdf redaction [in-a-comment]: a comment raises nothing\n",
            "/// [image-in-a-doc] is a doc mention, not a raise\n",
            "fn d() -> char { '{' }\n",
            "#[cfg(test)]\nmod fake;\n",
            "#[cfg(not(test))]\nmod real { fn r() -> &'static str { \"pdf region [not-test-is-production]: x\" } }\n",
            "#[cfg(all(test, feature = \"x\"))]\nmod gated {\n    fn g() -> &'static str { \"pdf redaction [only-in-gated-tests]: x\" }\n}\n",
            "#[cfg(test)]\nmod tests {\n",
            "    fn t() -> &'static str { \"pdf redaction [only-in-tests]: } { '}'\" }\n",
            "    fn u() -> &'static str { r#\"pdf region [raw-in-tests]: \"}\"#}\n",
            "}\n",
            "fn e() -> &'static str { \"pdf region [after-tests]: code after a test module\" }\n",
        );
        let mut ids = BTreeSet::new();
        let mut interpolated = Vec::new();
        let mut stamped = vec![0; STAMPED.len()];
        let files = scan(
            "core/x/src/planted.rs",
            source,
            &mut ids,
            &mut interpolated,
            &mut stamped,
        );
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        assert_eq!(
            ids,
            [
                "after-tests",
                "not-test-is-production",
                "planted-bracket",
                "plantedword"
            ]
        );
        assert_eq!(
            interpolated,
            ["core/x/src/planted.rs: pdf redaction [{PLANTED}]: x"]
        );
        assert_eq!(files, ["fake"]);
        // A STAMPED SITE IS ITS WHOLE LITERAL, IN ITS OWN FILE: the real one, folded across its
        // line continuation, is met; a new literal sharing its prefix is not admitted; and the
        // same literal in another file is not either.
        let real = "fn f() { format!(\"pdf redaction [{}]: the redacted region reaches {}, which the redaction removes \\\n                 no part of, so the region cannot be cleared\", a, b); }";
        let same_prefix =
            "fn g() { format!(\"pdf redaction [{}]: the redacted region reaches {} too\", a, b); }";
        let mut counted = vec![0; STAMPED.len()];
        let mut flagged = Vec::new();
        scan(
            "core/x/src/redact/steps.rs",
            real,
            &mut BTreeSet::new(),
            &mut flagged,
            &mut counted,
        );
        assert!(
            flagged.is_empty(),
            "the real stamped site was flagged: {flagged:?}"
        );
        assert_eq!(counted[0], 1, "the real stamped site was not met");
        scan(
            "core/x/src/redact/steps.rs",
            same_prefix,
            &mut BTreeSet::new(),
            &mut flagged,
            &mut counted,
        );
        assert_eq!(
            flagged.len(),
            1,
            "a new literal with a stamped prefix was admitted"
        );
        scan(
            "core/x/src/elsewhere.rs",
            real,
            &mut BTreeSet::new(),
            &mut flagged,
            &mut counted,
        );
        assert_eq!(
            flagged.len(),
            2,
            "a stamped literal was admitted outside its file"
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
