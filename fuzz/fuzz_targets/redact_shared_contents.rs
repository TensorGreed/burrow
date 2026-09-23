//! Fuzz the shared-`/Contents` detection over **generated documents**.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run redact_shared_contents -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # Why this target generates rather than parses
//!
//! Every other target here takes the fuzzer's bytes as a **document**. This one takes them as a
//! **shape**: how many pages, how many `/Contents` elements each has, and which slots point at
//! the same stream object. The document is then assembled correctly.
//!
//! That is deliberate and it is the lesson of the previous piece. A target fed arbitrary bytes
//! spends its budget on the parser's front door, and the question here is not whether the
//! parser survives — it is whether the **detection** is right across sharing structures nobody
//! would think to write by hand. `CLAUDE.md`: a harness that generates its own inputs is
//! measuring what it can generate, so what it generates is the thing under test.
//!
//! # The oracle, and why it is not the implementation written twice
//!
//! The generator **decides** the sharing, so it knows the answer before asking: it holds the
//! object number behind every slot, so the reference count of the element the region reaches is
//! arithmetic over its own construction. Nothing in the oracle reads `sharing.rs`.
//!
//! # The claim
//!
//! **If the redaction returns `Ok`, the element the region reached was referenced exactly
//! once.** One asymmetric claim, in the leak direction: refusing a page that could have been
//! redacted costs a document, and editing a stream another page is still using costs that
//! page's text with a clean read-back on the page that was asked for.
//!
//! Over-refusal is checked in the unit suite, where a fixture can be built to be sure the
//! region reaches what it is meant to. A fuzzer placing text from arbitrary bytes cannot say
//! that without re-deriving the geometry, which would be the walk written twice.

#![no_main]

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use libfuzzer_sys::fuzz_target;

/// `/Helvetica` widths for codes 32..=94, so the standard-14 refusal never fires and the target
/// measures sharing rather than measuring that.
const WIDTHS: &str = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
                      556 556 556 556 556 556 556 556 556 556 556 556 556]";

/// The shape the fuzzer's bytes describe.
struct Shape {
    /// For each page, the stream **slot index** each of its `/Contents` elements points at.
    /// Two slots holding the same number are two references to one object.
    pages: Vec<Vec<usize>>,
    /// How many distinct stream objects exist.
    streams: usize,
}

impl Shape {
    /// Read a shape from `data`, or `None` if it does not describe a usable one.
    fn read(data: &[u8]) -> Option<Self> {
        let mut bytes = data.iter().copied();
        let page_count = usize::from(bytes.next()? % 4) + 1;
        let streams = usize::from(bytes.next()? % 6) + 1;
        let mut pages = Vec::with_capacity(page_count);
        for _ in 0..page_count {
            let elements = usize::from(bytes.next()? % 3) + 1;
            let mut slots = Vec::with_capacity(elements);
            for _ in 0..elements {
                slots.push(usize::from(bytes.next()?) % streams);
            }
            pages.push(slots);
        }
        Some(Self { pages, streams })
    }

    /// How many references there are to the stream the region reaches, or `None` if the region
    /// reaches nothing on page 0.
    ///
    /// **The oracle.** Only stream 0 draws inside the region, so the element the cut lands in is
    /// whichever of page 0's elements points at slot 0 — **not** necessarily element 0, which
    /// is what the first version of this assumed. A shape whose page 0 is `[1, 0]` puts the
    /// drawn text in element 1, and an oracle reading element 0 would have counted references
    /// to the wrong stream and asserted about a slot the cut never touched.
    ///
    /// Counted over the whole construction, which is where the sharing was decided; nothing
    /// here reads the detection.
    fn references_to_the_reached_element(&self) -> Option<usize> {
        let first = self.pages.first()?;
        // Nothing on page 0 draws in the region, so there is no cut and no claim to make.
        if !first.contains(&0) {
            return None;
        }
        Some(self.pages.iter().flatten().filter(|slot| **slot == 0).count())
    }

    /// The document, byte-exact.
    fn build(&self) -> Vec<u8> {
        let mut objects: Vec<String> = Vec::new();
        // 1 catalog, 2 pages, 3 font, then the streams, then the page dictionaries.
        objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_owned());
        objects.push(String::new());
        objects.push(format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
             /FirstChar 32 /LastChar 94 /Widths {WIDTHS} >>"
        ));
        let first_stream = objects.len() + 1;
        for at in 0..self.streams {
            // ONLY STREAM 0 DRAWS IN THE REGION. Everything else draws well below it, so the
            // element the cut lands in is decided by construction rather than by geometry.
            let body = if at == 0 {
                "BT /F1 24 Tf 72 700 Td (SECRET) Tj ET\n".to_owned()
            } else {
                format!("BT /F1 12 Tf 72 {} Td (KIN) Tj ET\n", 100 + at * 10)
            };
            objects.push(format!(
                "<< /Length {} >>\nstream\n{body}endstream",
                body.len()
            ));
        }
        let first_page = objects.len() + 1;
        let mut kids = Vec::with_capacity(self.pages.len());
        for slots in &self.pages {
            kids.push(format!("{} 0 R", objects.len() + 1));
            let refs: Vec<String> = slots
                .iter()
                .map(|slot| format!("{} 0 R", first_stream + slot))
                .collect();
            let contents = if refs.len() == 1 {
                refs[0].clone()
            } else {
                format!("[{}]", refs.join(" "))
            };
            objects.push(format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {contents} >>"
            ));
        }
        let _ = first_page;
        objects[1] = format!(
            "<< /Type /Pages /Count {} /Kids [{}] >>",
            self.pages.len(),
            kids.join(" ")
        );

        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
        }
        let xref_at = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        ));
        out.into_bytes()
    }
}

fuzz_target!(|data: &[u8]| {
    let Some(shape) = Shape::read(data) else {
        return;
    };
    let document = shape.build();
    let region = Region {
        left: 40.0,
        top: 40.0,
        width: 500.0,
        height: 120.0,
    };
    let redacted: BTreeSet<usize> = [0].into_iter().collect();

    let Ok((out, _)) = burrow_engines::redact_probe::redact_page(&document, 0, redacted, region)
    else {
        // A refusal is an outcome. Which refusal is the unit suite's question, because a
        // fixture there can be built to be certain what the region reaches.
        return;
    };
    assert!(out.starts_with(b"%PDF"), "the output must be a PDF");

    // THE CLAIM. It accepted, so the element it edited must have been referenced once and only
    // once -- by construction, not by asking the code that decided to accept.
    let Some(references) = shape.references_to_the_reached_element() else {
        return;
    };
    assert!(
        references == 1,
        "the redaction accepted a page whose reached /Contents element is referenced \
         {references} times; editing it removes text from a page that was not selected"
    );
});
