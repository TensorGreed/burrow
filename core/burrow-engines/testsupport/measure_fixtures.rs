//! The two synthetic documents the measurement examples are timed against.
//!
//! **Shared rather than copied**, for the reason `minimal_pdf.rs` and `pdf_reading.rs` are:
//! `measure-verification` and `measure-pruning` both need a flat 10,000-page document and a
//! deeply-nested one, and two generators agree until the day they do not — at which point two
//! recorded measurements are being compared against different documents while appearing to be
//! comparable. The numbers from both examples land in ADRs side by side, so that is not a
//! hypothetical cost.
//!
//! Neither is a fixture and neither is committed: they are built in memory, at the size the
//! caller asks for, because the shapes that matter here are shapes the committed fixtures do
//! not have.

#![allow(
    dead_code,
    clippy::missing_docs_in_private_items,
    clippy::indexing_slicing,
    clippy::unwrap_used
)]

#[path = "minimal_pdf.rs"]
mod minimal_pdf;

/// A document of `pages` pages, for measuring the sweeps at a realistic ceiling.
pub fn generated(pages: usize) -> Vec<u8> {
    minimal_pdf::pdf_with_pages(pages)
}

/// The same page count, reached through a page tree `depth` nodes deep.
///
/// THE SHAPE THE COMMITTED FIXTURES DO NOT HAVE, and the one the promise sweep is
/// sensitive to: `effective_rotation` walks `/Parent` to the root per page, so the sweep
/// costs pages x depth while the write costs pages. Both numbers are attacker-chosen, and
/// on a flat tree the sweep looks like a rounding error -- which is how it came to sit
/// outside every deadline.
///
/// Object numbering: 1 catalogue, 2..=depth+1 the chain of `/Pages` nodes, then one
/// object per page hanging off the last of them.
pub fn deep(pages: usize, depth: usize) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let leaf = depth + 1;
    let page_obj = |i: usize| leaf + 1 + i;

    let object = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: String| {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    };

    object(
        &mut out,
        &mut offsets,
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_owned(),
    );

    for node in 2..=leaf {
        let kids = if node == leaf {
            (0..pages)
                .map(|i| format!("{} 0 R", page_obj(i)))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            format!("{} 0 R", node + 1)
        };
        let parent = if node == 2 {
            String::new()
        } else {
            format!(" /Parent {} 0 R", node - 1)
        };
        object(
            &mut out,
            &mut offsets,
            format!(
                "{node} 0 obj\n<< /Type /Pages{parent} /Kids [{kids}] /Count {pages} \
                 /MediaBox [0 0 612 792] >>\nendobj\n"
            ),
        );
    }

    for i in 0..pages {
        object(
            &mut out,
            &mut offsets,
            format!(
                "{} 0 obj\n<< /Type /Page /Parent {leaf} 0 R /Resources << >> \
                 /MediaBox [0 0 612 792] >>\nendobj\n",
                page_obj(i)
            ),
        );
    }

    let xref = out.len();
    let count = offsets.len() + 1;
    out.extend_from_slice(format!("xref\n0 {count}\n0000000000 65535 f \n").as_bytes());
    for at in &offsets {
        out.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}
