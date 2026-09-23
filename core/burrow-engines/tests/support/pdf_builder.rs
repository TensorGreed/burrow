//! A hand-built PDF, byte-exact, for tests that need a shape no producer writes.
//!
//! Object numbers are one-based and follow the order objects are pushed, so a fixture states
//! its own references. Byte-exact rather than produced by a library, in the style of
//! `testsupport/minimal_pdf.rs`: a fixture measures the shape it was written for, and a
//! library's optimiser rewriting it measures the optimiser.

/// The objects of a document, assembled into a file with an uncompressed xref table.
#[derive(Default)]
pub struct Builder {
    objects: Vec<Vec<u8>>,
}

impl Builder {
    /// A document with no objects yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve the next object number without writing it.
    ///
    /// For the forward references a page tree needs: `/Pages` names its kids and each kid names
    /// its parent, so one of the two has to be written knowing a number that does not exist yet.
    pub fn reserve(&mut self) -> usize {
        self.objects.push(Vec::new());
        self.objects.len()
    }

    /// Write the body of a reserved object.
    pub fn put(&mut self, at: usize, body: &str) {
        if let Some(slot) = self.objects.get_mut(at - 1) {
            *slot = body.as_bytes().to_vec();
        }
    }

    /// Append an object and return its number.
    pub fn add(&mut self, body: &str) -> usize {
        let at = self.reserve();
        self.put(at, body);
        at
    }

    /// Append an uncompressed stream object and return its number.
    pub fn stream(&mut self, extra: &str, data: &str) -> usize {
        let body = format!(
            "<< /Length {}{extra} >>\nstream\n{data}endstream",
            data.len()
        );
        self.add(&body)
    }

    /// The file, with `root` as the catalogue.
    #[must_use]
    pub fn build(&self, root: usize) -> Vec<u8> {
        let mut out = String::from("%PDF-1.7\n");
        let mut offsets = Vec::new();
        for (index, body) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!(
                "{} 0 obj\n{}\nendobj\n",
                index + 1,
                String::from_utf8_lossy(body)
            ));
        }
        let xref_at = out.len();
        out.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            self.objects.len() + 1
        ));
        for offset in &offsets {
            out.push_str(&format!("{offset:010} 00000 n \n"));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root {root} 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            self.objects.len() + 1
        ));
        out.into_bytes()
    }
}

/// `/Helvetica`'s widths for the codes these fixtures draw, declared in the file.
///
/// **Declared rather than inherited.** A standard-14 font with no `/Widths` has no advances in
/// the document at all, and burrow refuses it rather than guessing — which would turn every
/// assertion built on these fixtures into a refusal, and a refusal is not the outcome any of
/// them is about. Stating them is what a producer does.
pub const HELVETICA_WIDTHS: &str = "[556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
      556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 556 \
      556 556 556 556 556 556 556 556 556 556 556 556 556]";

/// A `/Helvetica` font dictionary declaring widths for codes 32 to 94.
#[must_use]
pub fn helvetica_with_widths() -> String {
    format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
         /FirstChar 32 /LastChar 94 /Widths {HELVETICA_WIDTHS} >>"
    )
}
