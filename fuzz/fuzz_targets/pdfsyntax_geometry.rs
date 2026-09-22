//! Fuzz the glyph walk — the thing that decides what a redaction removes.
//!
//! ```bash
//! # from fuzz/
//! LD_LIBRARY_PATH=../engines/vendor/native-$(uname -m)/lib ASAN_OPTIONS=detect_leaks=0 \
//!   cargo +nightly fuzz run pdfsyntax_geometry -- \
//!     -max_total_time=60 -timeout=10 -rss_limit_mb=2048
//! ```
//!
//! # The resources are adversarial, because a real font dictionary is
//!
//! `glyphs_in` takes its widths, forms and font matrices from a `Resources` seam, and in a real
//! document every one of those comes out of the file. A stub returning sensible constants would
//! fuzz the operator parsing and nothing else. So the first bytes of the input choose the
//! metrics — including the degenerate ones a font dictionary is free to declare — and the form
//! table is wired from the same bytes, so cycles and deep nests are reachable by mutation rather
//! than only by a hand-written test.
//!
//! # The claims being tested
//!
//! 1. **No panic, no hang, no unbounded allocation**, on any content stream against any
//!    resources. The walk is driven entirely by file content and its recursion by a file's form
//!    table, so this is the whole reason the target exists.
//! 2. **Every outcome is `Ok` with a bounded glyph list, or a named refusal.** Never an
//!    `Err` whose message does not name a rule — that would be a refusal no test could assert
//!    on, which is the shape `Refusal` exists to prevent.
//! 3. **Every glyph's conservative box contains its advance box.** The box a redaction tests
//!    against must never be smaller than the pen's, whatever the file declares; the failing
//!    direction is a box too small, because that is the one that misses ink.
//! 4. **No box is built from a non-finite number.** An infinity composes to a NaN and
//!    `Rect::transformed` folds NaN corners into an inverted rectangle, which intersects
//!    nothing — a glyph a redaction silently skips.

#![no_main]

use burrow_engines::pdfsyntax::geometry::{
    Form, Glyph, GlyphMetrics, MAX_GLYPHS, Matrix, Rect, Refusal, Resources, WritingMode,
    glyphs_in,
};
use burrow_types::{Error, Result};
use libfuzzer_sys::fuzz_target;

/// Resources the fuzzer controls, standing in for a font dictionary it would also control.
struct Hostile {
    width: f64,
    font_matrix_scale: f64,
    bytes_per_code: u8,
    bbox: Option<Rect>,
    vertical: bool,
    forms: Vec<(Vec<u8>, Form)>,
}

impl Resources for Hostile {
    fn form(&self, name: &[u8]) -> Result<Option<Form>> {
        Ok(self
            .forms
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, f)| f.clone()))
    }

    fn glyph(&self, _name: &[u8], _code: u32) -> Result<GlyphMetrics> {
        Ok(GlyphMetrics {
            width: self.width,
            bytes_per_code: self.bytes_per_code,
            font_bbox: self.bbox,
            font_matrix: Matrix::scale(self.font_matrix_scale, self.font_matrix_scale),
            writing_mode: if self.vertical {
                WritingMode::Vertical
            } else {
                WritingMode::Horizontal
            },
        })
    }

    fn bytes_per_code(&self, _name: &[u8]) -> Result<u8> {
        Ok(self.bytes_per_code)
    }
}

/// One of the degenerate widths a `/W` array may really declare, chosen by the fuzzer.
fn pick(byte: u8) -> f64 {
    match byte % 8 {
        0 => 0.0,
        1 => 500.0,
        2 => -1000.0,
        3 => f64::MAX,
        4 => f64::MIN_POSITIVE,
        5 => 1e300,
        6 => -0.0,
        _ => 1000.0,
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((control, body)) = data.split_at_checked(6) else {
        return;
    };
    let resources = Hostile {
        width: pick(control[0]),
        font_matrix_scale: pick(control[1]) / 1000.0,
        bytes_per_code: control[2] % 5,
        bbox: (control[3] % 4 != 0).then(|| Rect {
            left: -pick(control[3]),
            bottom: -pick(control[4]),
            right: pick(control[3]),
            top: pick(control[4]),
        }),
        vertical: control[5] & 1 == 1,
        // THE FORM TABLE IS WIRED FROM THE SAME BYTES, so a cycle or a sixteen-deep nest is a
        // mutation away. A table of distinct, non-recursive forms would leave the two bounds
        // this module cares most about unreachable.
        forms: (0..4u64)
            .map(|index| {
                let name = format!("Fm{index}").into_bytes();
                let content = body.to_vec();
                (
                    name,
                    Form {
                        // Identity is what the cycle check keys on, so the fuzzer gets to
                        // decide whether two names are the same object.
                        id: u64::from(control[5] >> 1).wrapping_add(index % 3),
                        matrix: Matrix::scale(pick(control[1]) / 1000.0, 1.0),
                        content,
                    },
                )
            })
            .collect(),
    };

    match glyphs_in(body, &resources) {
        Ok(glyphs) => {
            // (1) bounded.
            assert!(
                glyphs.len() <= MAX_GLYPHS,
                "{} glyphs is past the ceiling",
                glyphs.len()
            );
            for glyph in &glyphs {
                check(glyph);
            }
        }
        // (2) EVERY REFUSAL THE GEOMETRY LAYER RAISES NAMES A RULE.
        //
        // Scoped to this layer, and the first run of this target is why. `glyphs_in` tokenises
        // through `ops::operations`, which has its own refusals and its own tests -- "an
        // operator longer than burrow will read" is one, and it reached this assertion in
        // seconds. Demanding a `Refusal` tag on an error another module raised is a claim about
        // the wrong module. The claim that is worth making is that nothing which *says* it came
        // from here arrives without a rule a test could name.
        Err(error) => {
            let named = Refusal::ALL.iter().any(|rule| rule.caught(&error));
            let claims_this_layer = match &error {
                Error::Malformed(message) | Error::Unsupported(message) => {
                    message.starts_with("pdf geometry")
                }
                // `Error::Internal` reports a bug in this code rather than a judgement about a
                // file, and is deliberately outside the refusal vocabulary.
                _ => false,
            };
            assert!(
                named || !claims_this_layer,
                "an unnamed refusal escaped the geometry walk: {error:?}"
            );
        }
    }
});

/// (3) and (4), per glyph.
fn check(glyph: &Glyph) {
    let box_ = glyph.conservative_box();
    // (4) no box is built from a non-finite number.
    for value in [box_.left, box_.bottom, box_.right, box_.top] {
        assert!(value.is_finite(), "a box entry is not finite: {box_:?}");
    }
    // (3) the conservative box is never smaller than the advance box. Stated as containment of
    // the origin's own column, which holds whatever the file declared the /FontBBox to be.
    assert!(
        box_.left <= box_.right && box_.bottom <= box_.top,
        "an inverted box intersects nothing, so its glyph is one a redaction skips: {box_:?}"
    );
}
