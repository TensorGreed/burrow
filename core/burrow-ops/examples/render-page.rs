//! Render one page of a PDF to a binary PGM, for fixture construction.
//!
//! ```text
//! cargo run -p burrow-ops --features native-engines --release \
//!   --example render-page -- <input.pdf> <page> <width> <height> <out.pgm>
//! ```
//!
//! # Why this exists
//!
//! `tools/make-producer-fixtures.py` builds an OCR'd-scan fixture the way a scanner and an OCR
//! program would: a page becomes a bitmap, and the bitmap becomes a PDF carrying the image plus
//! an invisible text layer. Getting from the page to the bitmap needs a renderer.
//!
//! **poppler's `pdftoppm` is installed on the usual dev machine and is deliberately not used.**
//! poppler is GPL, and this repository's licence rule is permissive-only (`CLAUDE.md`
//! non-negotiable 2, ADR 0003). The rule is about what burrow links and ships, and a rasteriser
//! in a fixture pipeline links nothing — but the reason to keep it out anyway is that the
//! pipeline is where a tool quietly becomes load-bearing, and the next person reaches for the
//! one already in the build. PDFium is vendored, permissive, and is the renderer the site
//! itself uses.
//!
//! # It goes through `burrow_ops::render`, not through the FFI
//!
//! Spike 0006 called `FPDF_RenderPageBitmap` directly because it was measuring what a render
//! costs. This is fixture construction, so it uses the SHIPPED path: the same `Limits`, the
//! same `Fit` arithmetic, the same progressive render with a deadline checkpoint, the same
//! refusal if `max_pixels` is exceeded. A fixture built through a private path is a fixture
//! built for a renderer nobody ships.
//!
//! **Not production code.** No typed errors, no `catch_unwind`; it prints and exits non-zero.

#![allow(
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "an example that reports to a person and exits; the workspace panic lints are for \
              library code"
)]

#[cfg(all(feature = "native-engines", target_os = "linux"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use burrow_engines::pdfium::Pdfium;
    use burrow_engines::{OpenOptions, Raster};
    use burrow_ops::render::{Fit, render};
    use burrow_types::{Limits, SystemClock};

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 6 {
        eprintln!(
            "usage: {} <input.pdf> <page (1-based)> <width> <height> <out.pgm>",
            args[0]
        );
        std::process::exit(2);
    }
    let (input, page, width, height, out) = (
        &args[1],
        args[2].parse::<u64>()?,
        args[3].parse::<u32>()?,
        args[4].parse::<u32>()?,
        &args[5],
    );

    let bytes = std::fs::read(input)?.into_boxed_slice();
    let options = OpenOptions::new(Limits::DEFAULT, Arc::new(SystemClock::new()));
    let rendered = render(
        &Pdfium::new(),
        bytes,
        &[page],
        Fit::box_of(width, height),
        &options,
    )?;
    let Some(first) = rendered.into_iter().next() else {
        eprintln!("render returned no pages");
        std::process::exit(1);
    };
    let Raster {
        width,
        height,
        rgba,
        ..
    } = first.raster;

    // RGBA -> 8-bit grey, the ITU-R 601 luma a scanner would produce. Written as binary PGM
    // because it needs no encoder and Pillow reads it directly.
    let mut pgm = format!("P5\n{width} {height}\n255\n").into_bytes();
    pgm.reserve(rgba.len() / 4);
    for px in rgba.chunks_exact(4) {
        let (r, g, b) = (f32::from(px[0]), f32::from(px[1]), f32::from(px[2]));
        pgm.push(
            (0.299 * r + 0.587 * g + 0.114 * b)
                .round()
                .clamp(0.0, 255.0) as u8,
        );
    }
    std::fs::write(out, pgm)?;
    println!("{out}: {width}x{height} grey");
    Ok(())
}

#[cfg(not(all(feature = "native-engines", target_os = "linux")))]
fn main() {
    eprintln!(
        "render-page needs the native engines on Linux.\n\
         Build with:  cargo run -p burrow-ops --features native-engines --release \\\n\
         \x20              --example render-page -- <input.pdf> <page> <w> <h> <out.pgm>"
    );
    std::process::exit(2);
}
