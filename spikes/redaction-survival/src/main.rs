//! Spike 0006 — what survives a redaction.
//!
//! Runs four instruments over every fixture in THREE states, and reports per channel whether
//! the secret is still in the file, which instrument could tell, and whether a person would
//! still see it.
//!
//! ```text
//! cargo run --release --manifest-path spikes/redaction-survival/Cargo.toml -- [--mutate-redactor-to-noop]
//! ```
//!
//! # The three states, and why there are three rather than two
//!
//! | state | what it is |
//! |---|---|
//! | `fixture` | the document as `make-fixtures.py` built it |
//! | `roundtrip` | the same document opened and written by qpdf, with NO edit |
//! | `redacted` | the same, with the naive content-stream redaction applied |
//!
//! The middle one is not ceremony. The fixtures are uncompressed by construction, so a raw
//! scan of one is measuring the generator; qpdf's writer compresses, so the round-trip is the
//! first state that resembles an output. And on one channel the round-trip turns out to remove
//! the secret BY ITSELF, which a two-state harness would have credited to the redaction.
//!
//! # The controls, and why the run exits rather than warning
//!
//! 1. NON-VACUITY, keyed on the `fixture` state. A fixture no instrument can find the canary
//!    in is a broken fixture, and its "nothing survived" reads exactly like success.
//! 2. INERTNESS. `00-control-no-canary.pdf` must come back absent on every instrument.
//! 3. THE KEEP LINE, drawn outside the region in every fixture, must survive, or the
//!    measurement is of a deletion rather than a redaction.
//! 4. THE MUTATION. `--mutate-redactor-to-noop` makes the redactor do nothing; the harness
//!    asserts that it then removes zero text objects, because a mutation that did not apply is
//!    indistinguishable from a defence that holds.
//! 5. COUNTS. Every control prints n of N.

mod ffi;
mod fontscan;
mod pdfium;
mod qpdf;
mod redact;
mod scan;

use std::path::{Path, PathBuf};
// ---------------------------------------------------------------------------
// The record. Every table goes to stdout AND to a file the repository commits.
//
// WHY THE BINARY WRITES IT RATHER THAN A SHELL REDIRECT. The evidence this spike's report
// cites has to be an artifact somebody can diff, not a run they can repeat -- two wrong cost
// tables and a directory of no-op "redacted" PDFs in this very spike are the argument. A `>`
// in a documented command goes stale silently the first time somebody runs the binary without
// it; a file the binary writes cannot disagree with the run that produced it.
//
// It does NOT go in `results/`: `tools/check-no-generated-files.sh` refuses any tracked path
// matching `(^|/)results/`, and its own probe for that pattern is a spike results file from
// PR #37. That gate is correct and this is not the thing it is guarding against, so the
// record gets a directory whose name says what it is instead of an exception to the gate.
// ---------------------------------------------------------------------------

use std::cell::RefCell;

thread_local! {
    static RECORD: RefCell<String> = const { RefCell::new(String::new()) };
}

macro_rules! out {
    () => { out!("") };
    ($($arg:tt)*) => {{
        let line = format!($($arg)*);
        println!("{line}");
        RECORD.with(|r| {
            let mut r = r.borrow_mut();
            r.push_str(&line);
            r.push('\n');
        });
    }};
}

/// Write the accumulated record beside the sources, where git can see it.
fn write_record(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    RECORD.with(|r| std::fs::write(path, r.borrow().as_bytes())).map_err(|e| e.to_string())
}

use std::time::Instant;

const REGION: (f32, f32, f32, f32) = (30.0, 88.0, 370.0, 132.0);
const KEEP_LINE: &str = "KEEP-THIS-LINE";
const RENDER_SCALE: f32 = 4.0;
/// Below this, the region is blank enough that a person sees nothing there.
const INK_VISIBLE: f64 = 0.001;

struct Channel {
    number: u32,
    slug: &'static str,
}

macro_rules! channels {
    ($(($n:expr, $s:expr)),* $(,)?) => {
        &[$(Channel { number: $n, slug: $s }),*]
    };
}

const CHANNELS: &[Channel] = channels![
    (1, "plain-tj"),
    (2, "kerned-tj"),
    (3, "positioned-runs"),
    (4, "differences-encoding"),
    (5, "cid-with-tounicode"),
    (6, "cid-without-tounicode"),
    (7, "form-xobject"),
    (8, "type3-glyph"),
    (9, "actualtext"),
    (10, "structure-tree"),
    (11, "annotation"),
    (12, "acroform-field"),
    (13, "optional-content"),
    (14, "thumbnail"),
    (15, "metadata"),
    (16, "attachment"),
    (17, "incremental-update"),
    (18, "vector-outlines"),
    (19, "covered-by-a-rectangle"),
    (20, "image-pixels"),
    (21, "cid-lying-tounicode"),
    (22, "split-content-streams"),
    (24, "page-level-metadata"),
];

fn secret(channel: u32) -> String {
    format!("BURROW-SECRET-{channel:02}")
}

/// Channels whose subject is a carrier that is NOT the page get a second canary, placed only
/// in that carrier. Without it a removal cannot be attributed: channels 15 and 16 drew the
/// secret on the page as well, so when `split` dropped the catalogue-level object the scan
/// still found the page copy and the table read "carried it through".
fn carrier(channel: u32) -> Option<String> {
    matches!(channel, 9..=16 | 24).then(|| format!("BURROW-CARRIER-{channel:02}"))
}

#[derive(Debug, Default, Clone)]
struct State {
    i1_raw: bool,
    i2_qdf: bool,
    i3_text: bool,
    /// The same three, for the carrier canary where the channel has one.
    c1_raw: bool,
    c2_qdf: bool,
    c3_text: bool,
    has_carrier: bool,
    i4_ink: f64,
    i4_ink_with_annots: f64,
    thumb_present: bool,
    thumb_ink: f64,
    /// Channel 23: what the font objects still say. See `fontscan`.
    font: fontscan::FontLeak,
    text: String,
}

impl State {
    fn any_scan(&self) -> bool {
        self.i1_raw || self.i2_qdf || self.i3_text || self.any_carrier_scan()
    }
    fn any_carrier_scan(&self) -> bool {
        self.c1_raw || self.c2_qdf || self.c3_text
    }
    fn carrier_cell(&self, page_only: bool) -> &'static str {
        if !self.has_carrier {
            "—"
        } else if page_only {
            "n/a"
        } else if self.any_carrier_scan() {
            "**yes**"
        } else {
            "no"
        }
    }
    /// Ink in the region. NOT "the secret is visible" -- see `eyeball-verdicts.tsv`.
    fn inked(&self) -> bool {
        self.i4_ink > INK_VISIBLE
    }
    /// Is the secret still in the file at all?
    ///
    /// NOT the same question as "can an instrument see it", and the first version of this
    /// harness conflated them -- it computed this from the scans alone, and reported channel
    /// 20 (a secret in a JPEG, drawn on the page, plainly legible) as GONE. A carrier nothing
    /// can read is still a carrier.
    fn survives(&self, eyeball: Eyeball) -> bool {
        self.any_scan()
            || eyeball != Eyeball::No
            || (self.thumb_present && self.thumb_ink > 0.01)
            || self.font.any()
    }

    /// Everything this state observed, as one comparable tuple. Control 5 compares these.
    fn witness(&self) -> (bool, bool, bool, bool, bool, bool, bool) {
        (
            self.i1_raw,
            self.i2_qdf,
            self.i3_text,
            self.c1_raw,
            self.c2_qdf,
            self.c3_text,
            self.font.any(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eyeball {
    /// A person reading the output page reads the secret.
    Yes,
    /// Visible in a conforming viewer, but NOT in burrow's own render, which passes flags = 0
    /// and so does not draw annotation appearance streams.
    Viewer,
    No,
}

impl Eyeball {
    fn label(self) -> &'static str {
        match self {
            Eyeball::Yes => "yes",
            Eyeball::Viewer => "viewer only",
            Eyeball::No => "no",
        }
    }
}

/// Read the human verdicts. A channel with no row is an ERROR, not a default: a blank that
/// reads as "no" is the shape of every check in this repository that examined nothing.
fn load_eyeball(path: &Path) -> Result<std::collections::HashMap<(u32, String), Eyeball>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = std::collections::HashMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            return Err(format!("{}:{}: expected 3+ tab-separated columns", path.display(), n + 1));
        }
        let channel: u32 = cols[0]
            .trim()
            .parse()
            .map_err(|_| format!("{}:{}: bad channel", path.display(), n + 1))?;
        let verdict = match cols[2].trim() {
            "yes" => Eyeball::Yes,
            "viewer" => Eyeball::Viewer,
            "no" => Eyeball::No,
            other => {
                return Err(format!(
                    "{}:{}: verdict must be yes, viewer or no, not `{other}`",
                    path.display(),
                    n + 1
                ));
            }
        };
        out.insert((channel, cols[1].trim().to_string()), verdict);
    }
    Ok(out)
}

struct Outcome {
    fixture: State,
    roundtrip: State,
    redacted: State,
    keep_line_survives: bool,
    eyeball_roundtrip: Eyeball,
    eyeball_redacted: Eyeball,
    redaction: redact::Report,
    write_note: String,
    errors: Vec<String>,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("SPIKE_REPO_ROOT"))
}

fn qpdf_cli() -> PathBuf {
    let arch = std::env::consts::ARCH;
    let vendored = repo_root()
        .join("engines/vendor/src")
        .join(format!("build-qpdf-plain-{arch}"))
        .join("qpdf/qpdf");
    if vendored.is_file() {
        return vendored;
    }
    PathBuf::from("qpdf")
}

/// Open, redact the page's content, write back.
fn redact_document(
    bytes: &[u8],
    mutate: bool,
) -> Result<(Vec<u8>, redact::Report, String), String> {
    let doc = qpdf::Doc::open(bytes.to_vec())?;
    if doc.pages() < 1 {
        return Err("no pages".into());
    }
    let page = doc.page(0);
    let content = doc.page_content(page)?;
    let (rewritten, report) = redact::naive(&content, REGION, mutate);

    let streams = doc.content_streams(page);
    if streams.is_empty() {
        return Err("the page has no content stream to write back to".into());
    }
    let null = doc.null_via_missing_key(page);
    doc.replace_stream(streams[0], &rewritten, null);
    // `qpdf_oh_get_page_content_data` CONCATENATES, so by the time there is something to edit
    // the boundaries between the page's streams are gone. The only way to put the rewritten
    // bytes back is to collapse them into the first stream and empty the rest.
    let note = if streams.len() > 1 {
        for extra in &streams[1..] {
            doc.replace_stream(*extra, b"", null);
        }
        format!(
            "collapsed {} content streams into one; the array structure did not survive",
            streams.len()
        )
    } else {
        format!(
            "1 content stream, {} -> {} bytes",
            content.len(),
            rewritten.len()
        )
    };
    let out = doc.write()?;
    Ok((out, report, note))
}

fn measure(
    qpdf_path: &Path,
    scratch: &Path,
    bytes: &[u8],
    needle: &str,
    carrier_needle: Option<&str>,
    ppm: Option<&Path>,
    errors: &mut Vec<String>,
) -> State {
    let mut s = State {
        i1_raw: scan::raw(bytes, needle),
        c1_raw: carrier_needle.is_some_and(|c| scan::raw(bytes, c)),
        has_carrier: carrier_needle.is_some(),
        ..Default::default()
    };
    match scan::expand(qpdf_path, bytes, scratch) {
        Ok(expanded) => {
            s.i2_qdf = scan::raw(&expanded, needle);
            s.c2_qdf = carrier_needle.is_some_and(|c| scan::raw(&expanded, c));
            // Channel 23, on the SAME expanded bytes the scans use, so no extra parse.
            s.font = fontscan::scan(&expanded, needle);
        }
        // NO FALLBACK. Reported as an error, never as "not found".
        Err(e) => errors.push(format!("I2 unavailable: {e}")),
    }
    match pdfium::Document::open(bytes.to_vec()) {
        Ok(doc) => match doc.page(0) {
            Ok(page) => {
                match pdfium::text(&page) {
                    Ok((t, _chars)) => {
                        s.i3_text = t.contains(needle);
                        s.c3_text = carrier_needle.is_some_and(|c| t.contains(c));
                        s.text = t;
                    }
                    Err(e) => errors.push(format!("I3: {e}")),
                }
                match pdfium::render(&page, RENDER_SCALE, 0) {
                    Ok(raster) => {
                        s.i4_ink = raster.ink_in(REGION, page.height, RENDER_SCALE);
                        if let Some(path) = ppm {
                            let _ = std::fs::write(path, raster.to_ppm());
                        }
                    }
                    Err(e) => errors.push(format!("I4: {e}")),
                }
                // The same render with annotation appearances drawn. The difference between
                // the two is the difference between what burrow shows a person and what a
                // viewer shows them.
                match pdfium::render(&page, RENDER_SCALE, ffi::FPDF_ANNOT) {
                    Ok(raster) => {
                        s.i4_ink_with_annots = raster.ink_in(REGION, page.height, RENDER_SCALE);
                        if let Some(path) = ppm {
                            let annot_path = path.with_file_name(format!(
                                "{}-annots.ppm",
                                path.file_stem().unwrap_or_default().to_string_lossy()
                            ));
                            let _ = std::fs::write(annot_path, raster.to_ppm());
                        }
                    }
                    Err(e) => errors.push(format!("I4 (FPDF_ANNOT): {e}")),
                }
            }
            Err(e) => errors.push(format!("page 0: {e}")),
        },
        Err(e) => errors.push(format!("PDFium open: {e}")),
    }
    if let Ok(doc) = qpdf::Doc::open(bytes.to_vec()) {
        if doc.pages() >= 1 {
            let t = doc.thumbnail(doc.page(0));
            s.thumb_present = t.present;
            s.thumb_ink = if t.ink.is_nan() { 0.0 } else { t.ink };
            if t.ink.is_nan() {
                errors.push("thumbnail present but qpdf could not decode it".into());
            }
            // WRITE THE THUMBNAIL OUT. The report cited `results/14-thumb.png` before the
            // harness produced any such file -- review caught it. An artifact a document names
            // and the tree does not contain is a claim wearing a filename.
            if t.present && !t.pixels.is_empty() && t.width > 0 && t.height > 0 {
                if let Some(path) = ppm {
                    let out = path.with_file_name(format!(
                        "{}-thumb.pgm",
                        path.file_stem().unwrap_or_default().to_string_lossy()
                    ));
                    let mut pgm = format!("P5\n{} {}\n255\n", t.width, t.height).into_bytes();
                    pgm.extend_from_slice(&t.pixels);
                    let _ = std::fs::write(out, pgm);
                }
            }
        }
    }
    s
}

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn shown(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(60)
        .collect()
}

fn main() -> Result<(), String> {
    let mutate = std::env::args().any(|a| a == "--mutate-redactor-to-noop");
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixtures = here.join("results/fixtures");
    let out_dir = here.join("results");
    // A1. THE ARTIFACT DIRECTORY VARIES WITH THE MUTATION, and it did not until review found
    // it. `pages_dir` was already split; the redacted PDFs were not, so running the mutation
    // second -- which is the order this spike's own README gives -- overwrote every real
    // redacted output with the output of a redactor that does nothing. In a spike about files
    // that look redacted and are not, the artifact directory was one.
    let pages_dir = out_dir.join(if mutate { "pages-mutated" } else { "pages" });
    let pdf_dir = out_dir.join(if mutate { "redacted-mutated" } else { "redacted" });
    std::fs::create_dir_all(&pdf_dir).map_err(|e| e.to_string())?;
    let scratch = out_dir.join("scratch");
    std::fs::create_dir_all(&pages_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    if !fixtures.is_dir() {
        return Err(format!(
            "{} is missing. Run: python3 spikes/redaction-survival/make-fixtures.py \
             spikes/redaction-survival/results/fixtures",
            fixtures.display()
        ));
    }
    let qpdf_path = qpdf_cli();
    pdfium::init();
    let eyeball = load_eyeball(&here.join("eyeball-verdicts.tsv"))?;
    let missing: Vec<String> = CHANNELS
        .iter()
        .flat_map(|c| ["roundtrip", "redacted"].map(move |st| (c, st)))
        .filter(|(c, st)| !eyeball.contains_key(&(c.number, (*st).to_string())))
        .map(|(c, st)| format!("{}/{st}", c.number))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "eyeball-verdicts.tsv has no row for: {}. Look at results/pages/*.ppm and write \
             one -- a missing row must not default to `no`.",
            missing.join(", ")
        ));
    }

    out!("spike 0006 — what survives a redaction");
    out!(
        "region {REGION:?}, render scale {RENDER_SCALE}x, ink threshold {INK_VISIBLE}, \
         qpdf {}",
        qpdf_path.display()
    );
    if mutate {
        out!("\n*** CONTROL 4: the redactor is mutated to a no-op ***");
    }

    // ---- control 2: inertness -------------------------------------------------------
    let control_raw = std::fs::read(fixtures.join("00-control-no-canary.pdf"))
        .map_err(|e| e.to_string())?;
    let control_bytes = qpdf::roundtrip(&control_raw)?;
    let mut inert_failures = Vec::new();
    let mut errors = Vec::new();
    for channel in CHANNELS {
        let needle = secret(channel.number);
        let s = measure(
            &qpdf_path,
            &scratch,
            &control_bytes,
            &needle,
            carrier(channel.number).as_deref(),
            None,
            &mut errors,
        );
        if s.any_scan() {
            inert_failures.push(format!("{needle} matched the canary-free control"));
        }
    }
    if !errors.is_empty() {
        return Err(format!("control 2 could not run: {}", errors.join("; ")));
    }
    out!(
        "\ncontrol 2 (inertness): {} of {} canaries absent from the canary-free control",
        CHANNELS.len() - inert_failures.len(),
        CHANNELS.len()
    );
    for f in &inert_failures {
        eprintln!("  FAIL {f}");
    }

    // ---- the run --------------------------------------------------------------------
    let mut outcomes: Vec<Outcome> = Vec::new();
    let mut vacuity_failures = Vec::new();

    for channel in CHANNELS {
        let path = fixtures.join(format!("{:02}-{}.pdf", channel.number, channel.slug));
        let raw = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let needle = secret(channel.number);
        let mut errors = Vec::new();

        let eye_rt = eyeball[&(channel.number, "roundtrip".to_string())];
        // Under the mutation the redactor does nothing, so the "redacted" document IS the
        // round-trip and the round-trip's human verdict is the one that describes it. Reading
        // the `redacted` row there instead compares this run against a verdict written about a
        // different document -- which is what the first version of this control did, and it
        // failed three channels for that reason rather than for a real one.
        let eye_rd = if mutate {
            eye_rt
        } else {
            eyeball[&(channel.number, "redacted".to_string())]
        };

        let carrier_needle = carrier(channel.number);
        let fixture = measure(
            &qpdf_path,
            &scratch,
            &raw,
            &needle,
            carrier_needle.as_deref(),
            None,
            &mut errors,
        );
        // CONTROL 1, keyed on the fixture as built.
        if !fixture.survives(eye_rt) {
            vacuity_failures.push(format!(
                "channel {:02} {}: no instrument finds the canary in the fixture itself",
                channel.number, channel.slug
            ));
        }

        let baseline = qpdf::roundtrip(&raw)?;
        let roundtrip = measure(
            &qpdf_path,
            &scratch,
            &baseline,
            &needle,
            carrier_needle.as_deref(),
            Some(&pages_dir.join(format!("{:02}-roundtrip.ppm", channel.number))),
            &mut errors,
        );

        let (redacted_bytes, report, write_note) = redact_document(&raw, mutate)?;
        let redacted = measure(
            &qpdf_path,
            &scratch,
            &redacted_bytes,
            &needle,
            carrier_needle.as_deref(),
            Some(&pages_dir.join(format!("{:02}-redacted.ppm", channel.number))),
            &mut errors,
        );
        let keep = scan::raw(&redacted_bytes, KEEP_LINE)
            || redacted.text.contains(KEEP_LINE)
            || scan::expand(&qpdf_path, &redacted_bytes, &scratch)
                .map(|e| scan::raw(&e, KEEP_LINE))
                .unwrap_or(false);
        std::fs::write(
            pdf_dir.join(format!("redacted-{:02}-{}.pdf", channel.number, channel.slug)),
            &redacted_bytes,
        )
        .map_err(|e| e.to_string())?;

        outcomes.push(Outcome {
            fixture,
            roundtrip,
            redacted,
            keep_line_survives: keep,
            eyeball_roundtrip: eye_rt,
            eyeball_redacted: eye_rd,
            redaction: report,
            write_note,
            errors,
        });
    }

    out!(
        "control 1 (non-vacuity): {} of {} fixtures carry a canary something can find",
        CHANNELS.len() - vacuity_failures.len(),
        CHANNELS.len()
    );
    for f in &vacuity_failures {
        eprintln!("  FAIL {f}");
    }

    // ---- the tables -----------------------------------------------------------------
    out!("\n## Finding 1 — what the naive redaction leaves behind");
    out!();
    out!(
        "| # | channel | text objects cut | still in the file? | a person sees it? | removed by the qpdf round-trip alone? |"
    );
    out!("|--:|---|--:|:--:|:--:|:--:|");
    for (c, o) in CHANNELS.iter().zip(&outcomes) {
        out!(
            "| {} | {} | {}/{} | **{}** | **{}** | {} |",
            c.number,
            c.slug,
            o.redaction.text_objects_removed,
            o.redaction.text_objects,
            yn(o.redacted.survives(o.eyeball_redacted)),
            o.eyeball_redacted.label(),
            yn(o.fixture.survives(o.eyeball_roundtrip)
                && !o.roundtrip.survives(o.eyeball_roundtrip)),
        );
    }

    out!("\n## Finding 2 — which instrument can see it, after the redaction");
    out!();
    out!(
        "| # | channel | I1 raw | I2 --qdf | I3 PDFium text | carrier canary | **the font (ch 23)** | I4 ink, flags=0 | I4 ink, FPDF_ANNOT | /Thumb ink |"
    );
    out!("|--:|---|:--:|:--:|:--:|:--:|:--:|--:|--:|--:|");
    for (c, o) in CHANNELS.iter().zip(&outcomes) {
        out!(
            "| {} | {} | {} | {} | {} | {} | {} | {:.4} | {:.4} | {} |",
            c.number,
            c.slug,
            yn(o.redacted.i1_raw),
            yn(o.redacted.i2_qdf),
            yn(o.redacted.i3_text),
            o.redacted.carrier_cell(false),
            o.redacted.font.cell(),
            o.redacted.i4_ink,
            o.redacted.i4_ink_with_annots,
            if o.redacted.thumb_present {
                format!("{:.3}", o.redacted.thumb_ink)
            } else {
                "—".into()
            },
        );
    }

    out!("\n### The same instruments on the round-trip, before any redaction");
    out!();
    out!("| # | channel | I1 | I2 | I3 | I4 ink, flags=0 | I4 ink, FPDF_ANNOT | a person sees it | keep line survived |");
    out!("|--:|---|:--:|:--:|:--:|--:|--:|:--:|:--:|");
    for (c, o) in CHANNELS.iter().zip(&outcomes) {
        out!(
            "| {} | {} | {} | {} | {} | {:.4} | {:.4} | {} | {} |",
            c.number,
            c.slug,
            yn(o.roundtrip.i1_raw),
            yn(o.roundtrip.i2_qdf),
            yn(o.roundtrip.i3_text),
            o.roundtrip.i4_ink,
            o.roundtrip.i4_ink_with_annots,
            o.eyeball_roundtrip.label(),
            yn(o.keep_line_survives),
        );
    }

    out!("\n### What PDFium's text page reads, before and after");
    out!();
    out!("| # | channel | before | after |");
    out!("|--:|---|---|---|");
    for (c, o) in CHANNELS.iter().zip(&outcomes) {
        out!(
            "| {} | {} | `{}` | `{}` |",
            c.number,
            c.slug,
            shown(&o.roundtrip.text),
            shown(&o.redacted.text)
        );
    }

    out!("\n### Writing the content stream back");
    out!();
    out!("| # | channel | note |");
    out!("|--:|---|---|");
    for (c, o) in CHANNELS.iter().zip(&outcomes) {
        out!("| {} | {} | {} |", c.number, c.slug, o.write_note);
    }

    let with_errors: Vec<_> = CHANNELS
        .iter()
        .zip(&outcomes)
        .filter(|(_, o)| !o.errors.is_empty())
        .collect();
    if !with_errors.is_empty() {
        out!("\n### Instrument errors, reported rather than counted as `not found`");
        for (c, o) in &with_errors {
            out!("- channel {}: {}", c.number, o.errors.join("; "));
        }
    }

    // ---- control 5: a channel scored GONE must have been OBSERVED to go ---------------
    //
    // ADDED AFTER REVIEW, because this is the hole the other four left and channel 23 is it
    // firing. `survives()` is a disjunction of witnesses, so a channel where every instrument
    // is blind and the human wrote "no" scores "not in the file" with no evidence that it
    // went. Control 1 cannot catch it: it keys on the FIXTURE, where the raw scan fires on
    // uncompressed generator output. Control 4 cannot catch it: a no-op leaves both states
    // equally blind, so they agree.
    //
    // The rule: if the redacted state is scored GONE, something must have CHANGED between the
    // round-trip and it. A channel that measures identically to the round-trip and is reported
    // gone is a channel nothing observed.
    let unobserved: Vec<u32> = CHANNELS
        .iter()
        .zip(&outcomes)
        .filter(|(_, o)| {
            !o.redacted.survives(o.eyeball_redacted)
                // A channel already gone at the round-trip HAS a named witness: the
                // fixture-to-round-trip transition, which finding 1's last column reports.
                // It is "observed earlier", not "never observed".
                && o.roundtrip.survives(o.eyeball_roundtrip)
                && o.redacted.witness() == o.roundtrip.witness()
                && o.eyeball_redacted == o.eyeball_roundtrip
        })
        .map(|(c, _)| c.number)
        .collect();
    out!(
        "control 5 (a removal was observed): {} of {} channels scored gone were seen to go",
        outcomes
            .iter()
            .filter(|o| !o.redacted.survives(o.eyeball_redacted))
            .count()
            - unobserved.len(),
        outcomes
            .iter()
            .filter(|o| !o.redacted.survives(o.eyeball_redacted))
            .count()
    );
    if !unobserved.is_empty() && !mutate {
        for n in &unobserved {
            eprintln!(
                "  FAIL channel {n}: reported gone, but every instrument measures it exactly as \
                 the round-trip did. Nothing observed a removal."
            );
        }
    }

    // ---- control 3 ------------------------------------------------------------------
    let kept = outcomes.iter().filter(|o| o.keep_line_survives).count();
    out!(
        "\ncontrol 3 (the keep line): survived in {} of {} channels",
        kept,
        outcomes.len()
    );

    // ---- control 4 ------------------------------------------------------------------
    if mutate {
        let surviving = outcomes
            .iter()
            .filter(|o| o.redacted.survives(o.eyeball_redacted))
            .count();
        let cut: usize = outcomes
            .iter()
            .map(|o| o.redaction.text_objects_removed)
            .sum();
        out!(
            "control 4 (mutation): {surviving} of {} channels still carry the secret, and the \
             run cut {cut} text objects",
            outcomes.len()
        );
        if cut != 0 {
            return Err("the no-op mutation still cut text objects; it did not apply".into());
        }
        // THE SHARP FORM OF THIS CONTROL. Not "everything survives" -- channel 17's secret is
        // genuinely gone before the redactor runs, because qpdf's write discards the
        // superseded object, and demanding 22 of 22 would need an exception list. What must
        // hold is that a no-op redaction leaves the measurement IDENTICAL to the round-trip's,
        // for every channel. Anything else means the redacted-state numbers are not being
        // driven by the redactor.
        let disagree: Vec<u32> = CHANNELS
            .iter()
            .zip(&outcomes)
            .filter(|(_, o)| {
                o.redacted.survives(o.eyeball_redacted)
                    != o.roundtrip.survives(o.eyeball_roundtrip)
                    || o.redacted.i1_raw != o.roundtrip.i1_raw
                    || o.redacted.i2_qdf != o.roundtrip.i2_qdf
                    || o.redacted.i3_text != o.roundtrip.i3_text
            })
            .map(|(c, _)| c.number)
            .collect();
        out!(
            "control 4 (mutation): {} of {} channels measure identically to the round-trip",
            outcomes.len() - disagree.len(),
            outcomes.len()
        );
        if !disagree.is_empty() {
            return Err(format!(
                "a no-op redaction changed the measurement on channels {disagree:?}; the \
                 redacted-state numbers are not driven by the redactor"
            ));
        }
    }

    // ---- instrument cost ------------------------------------------------------------
    out!("\n## Instrument cost — medians of 11");
    out!();
    out!("| document | pages | I1 raw | I2 --qdf | I3 PDFium text | I4 render {RENDER_SCALE}x |");
    out!("|---|--:|--:|--:|--:|--:|");
    for (label, path) in [
        (
            "01-plain-tj.pdf, round-tripped (1 page)".to_string(),
            fixtures.join("01-plain-tj.pdf"),
        ),
        (
            "tests/conformance/fixtures/pages-137.pdf".to_string(),
            repo_root().join("tests/conformance/fixtures/pages-137.pdf"),
        ),
    ] {
        if !path.is_file() {
            out!("| {label} | — | not present | | | |");
            continue;
        }
        let bytes = qpdf::roundtrip(&std::fs::read(&path).map_err(|e| e.to_string())?)?;
        let pages = pdfium::Document::open(bytes.clone())
            .map(|d| d.pages())
            .unwrap_or(-1);
        let t1 = median(11, || {
            let _ = scan::raw(&bytes, "BURROW-SECRET-01");
        });
        let t2 = median(11, || {
            let _ = scan::expand(&qpdf_path, &bytes, &scratch);
        });
        let t3 = median(11, || {
            if let Ok(doc) = pdfium::Document::open(bytes.clone()) {
                for i in 0..doc.pages() {
                    if let Ok(p) = doc.page(i) {
                        let _ = pdfium::text(&p);
                    }
                }
            }
        });
        let t4 = median(11, || {
            if let Ok(doc) = pdfium::Document::open(bytes.clone()) {
                for i in 0..doc.pages() {
                    if let Ok(p) = doc.page(i) {
                        let _ = pdfium::render(&p, RENDER_SCALE, 0);
                    }
                }
            }
        });
        out!(
            "| {label} | {pages} | {} | {} | {} | {} |",
            ms(t1),
            ms(t2),
            ms(t3),
            ms(t4)
        );
    }

    // ---- finding 4: the write verb's operands ---------------------------------------
    out!("\n## Finding 4 — the write verb's operands");
    let probe = std::fs::read(fixtures.join("01-plain-tj.pdf")).map_err(|e| e.to_string())?;
    {
        let doc = qpdf::Doc::open(probe)?;
        let page = doc.page(0);
        let via_key = doc.null_via_missing_key(page);
        let via_ctor = doc.null_via_constructor();
        out!(
            "- a missing key through the TRAPPED `qpdf_oh_get_key` yields type `{}`, unparsing as `{}`",
            doc.type_name(via_key),
            doc.unparse(via_key)
        );
        out!(
            "- the UNTRAPPED `qpdf_oh_new_null` yields type `{}`, unparsing as `{}`",
            doc.type_name(via_ctor),
            doc.unparse(via_ctor)
        );
        out!(
            "- interchangeable for `qpdf_oh_replace_stream_data`: {}",
            yn(doc.type_name(via_key) == doc.type_name(via_ctor))
        );
    }

    out!();
    out!("### What is reachable from a page handle, and what is not");
    out!();
    out!("| # | channel | the carrier | reachable from `qpdf_get_page_n`? |");
    out!("|--:|---|---|:--:|");
    for (number, slug, key, where_it_lives) in [
        (10u32, "structure-tree", "/StructTreeRoot", "the catalogue"),
        (11, "annotation", "/Annots", "the page"),
        (12, "acroform-field", "/AcroForm", "the catalogue"),
        (13, "optional-content", "/OCProperties", "the catalogue"),
        (14, "thumbnail", "/Thumb", "the page"),
        (15, "metadata", "/Metadata", "the catalogue"),
        (16, "attachment", "/Names", "the catalogue"),
    ] {
        let path = fixtures.join(format!("{number:02}-{slug}.pdf"));
        let Ok(bytes) = std::fs::read(&path) else { continue };
        let Ok(doc) = qpdf::Doc::open(bytes) else { continue };
        let page = doc.page(0);
        let found = doc.page_key_type(page, key);
        out!(
            "| {number} | {slug} | `{key}`, on {where_it_lives} | {} |",
            if found == "null" {
                "**no** — and `qpdf_get_root` is not callable (ADR 0013)".to_string()
            } else {
                format!("yes, as a `{found}`")
            }
        );
    }

    if !vacuity_failures.is_empty() || !inert_failures.is_empty() {
        return Err(format!(
            "{} non-vacuity and {} inertness failures; the tables above measure nothing for those rows",
            vacuity_failures.len(),
            inert_failures.len()
        ));
    }
    if !unobserved.is_empty() && !mutate {
        return Err(format!(
            "control 5: channels {unobserved:?} are reported gone and nothing observed them go"
        ));
    }
    out!();
    out!("artifacts in {}", out_dir.display());
    let record = here.join("measurements").join(if mutate {
        "run-mutated.txt"
    } else {
        "run.txt"
    });
    write_record(&record)?;
    eprintln!("record written to {}", record.display());
    Ok(())
}

fn median(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut times: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times[runs / 2]
}

fn ms(v: f64) -> String {
    if v < 1.0 {
        format!("{:.0} µs", v * 1000.0)
    } else {
        format!("{v:.2} ms")
    }
}
