//! A page rotation, normalised on the way in.

use crate::{Error, Result};

/// A quarter-turn rotation, reduced to one of four values.
///
/// # Why a type rather than an `i32`
///
/// `/Rotate` in a PDF is "the number of degrees by which the page shall be rotated clockwise
/// when displayed", and the specification requires a multiple of 90 — but it says nothing
/// about the range, and real files carry `-90`, `270` and `450` for the same quarter turn.
/// Every one of those has to mean the same thing here, and every non-multiple has to be
/// refused. Doing that at the boundary, once, is what stops the four representations
/// spreading: past this type there is exactly one spelling of each turn.
///
/// # What is accepted
///
/// Any multiple of 90, positive or negative, of any magnitude. `-90`, `270` and `990` all
/// reduce to [`Rotation::Clockwise270`]; `0`, `360` and `-720` all reduce to
/// [`Rotation::None`]. Anything else is [`Error::InvalidArgument`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Rotation {
    /// No rotation. `/Rotate 0`, or no `/Rotate` at all.
    None,
    /// A quarter turn clockwise. `/Rotate 90`.
    Clockwise90,
    /// A half turn. `/Rotate 180`.
    Clockwise180,
    /// A quarter turn anticlockwise. `/Rotate 270`.
    Clockwise270,
}

impl Rotation {
    /// Reduce any multiple of 90 to one of the four turns.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `degrees` is not a multiple of 90. The message names the
    /// value, which is a caller's argument rather than anything read out of a file — nothing
    /// derived from file content may reach an error (`core/CLAUDE.md`).
    pub fn from_degrees(degrees: i64) -> Result<Self> {
        if degrees % 90 != 0 {
            return Err(Error::InvalidArgument(format!(
                "rotation must be a multiple of 90 degrees, got {degrees}"
            )));
        }
        // `rem_euclid` rather than `%`: Rust's `%` keeps the sign of the dividend, so
        // `-90 % 360` is `-90` and a straight match would miss three of the four negative
        // turns. Euclidean remainder is always in `0..360`, which is the whole point of
        // normalising here.
        match degrees.rem_euclid(360) {
            0 => Ok(Self::None),
            90 => Ok(Self::Clockwise90),
            180 => Ok(Self::Clockwise180),
            270 => Ok(Self::Clockwise270),
            // Unreachable: a multiple of 90 reduced modulo 360 is one of those four. Written
            // as a typed error rather than an `unreachable!()` because library code here
            // does not panic, and an arithmetic assumption that is wrong should say so rather
            // than abort a process holding somebody's document.
            other => Err(Error::Internal(format!(
                "rotation normalisation produced {other}, which is not a quarter turn"
            ))),
        }
    }

    /// The value to write to `/Rotate`: 0, 90, 180 or 270.
    #[must_use]
    pub const fn degrees(self) -> i64 {
        match self {
            Self::None => 0,
            Self::Clockwise90 => 90,
            Self::Clockwise180 => 180,
            Self::Clockwise270 => 270,
        }
    }

    /// This rotation applied on top of `base`.
    ///
    /// Rotation is relative: a page that already carries `/Rotate 90` and is turned another
    /// 90 ends at 180. A caller wanting an absolute setting uses the rotation directly.
    #[must_use]
    pub fn after(self, base: Self) -> Self {
        // Both are quarter turns, so the sum is a multiple of 90 and cannot fail. Matched
        // rather than unwrapped, because `from_degrees` returns a `Result` and this crate
        // does not unwrap.
        match Self::from_degrees(self.degrees() + base.degrees()) {
            Ok(rotation) => rotation,
            // Unreachable for the same reason as above; `None` is the identity, which is the
            // one answer that cannot make a document wrong in a way nobody notices.
            Err(_) => Self::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Rotation;
    use crate::Error;

    #[test]
    fn the_four_turns_round_trip() {
        for degrees in [0, 90, 180, 270] {
            let rotation = Rotation::from_degrees(degrees).unwrap();
            assert_eq!(rotation.degrees(), degrees);
        }
    }

    #[test]
    fn negatives_and_values_over_360_reduce() {
        let cases: [(i64, i64); 10] = [
            (-90, 270),
            (-180, 180),
            (-270, 90),
            (-360, 0),
            (-450, 270),
            (360, 0),
            (450, 90),
            (720, 0),
            (990, 270),
            (-720, 0),
        ];
        for (given, expected) in cases {
            let rotation = Rotation::from_degrees(given).unwrap();
            assert_eq!(
                rotation.degrees(),
                expected,
                "{given} degrees should reduce to {expected}"
            );
        }
    }

    #[test]
    fn anything_that_is_not_a_quarter_turn_is_refused() {
        for degrees in [1, 45, 89, 91, -45, 100, 359, i64::MAX, i64::MIN + 1] {
            assert!(
                matches!(
                    Rotation::from_degrees(degrees),
                    Err(Error::InvalidArgument(_))
                ),
                "{degrees} degrees should be refused"
            );
        }
    }

    #[test]
    fn the_extremes_of_i64_do_not_overflow() {
        // `i64::MIN` is not a multiple of 90, so it is refused before any arithmetic. The
        // case is here because `rem_euclid` on `i64::MIN` is the kind of thing that panics
        // when the guard above it moves.
        assert!(Rotation::from_degrees(i64::MIN).is_err());
        // The largest multiple of 90 that fits. `i64::MAX` is 9_223_372_036_854_775_807;
        // 9_223_372_036_854_775_710 is 90 x 102_481_911_520_608_619. Written out rather than
        // computed because `integer_division` is denied workspace-wide, and the reason it is
        // denied -- silent loss of precision on a value that came from a file -- is exactly
        // what a reader should not have to re-derive here.
        let largest: i64 = 9_223_372_036_854_775_710;
        assert!(Rotation::from_degrees(largest).is_ok());
        assert!(Rotation::from_degrees(-largest).is_ok());
    }

    #[test]
    fn rotation_composes() {
        assert_eq!(
            Rotation::Clockwise90.after(Rotation::Clockwise90),
            Rotation::Clockwise180
        );
        assert_eq!(
            Rotation::Clockwise270.after(Rotation::Clockwise90),
            Rotation::None
        );
        assert_eq!(
            Rotation::None.after(Rotation::Clockwise180),
            Rotation::Clockwise180
        );
    }
}
