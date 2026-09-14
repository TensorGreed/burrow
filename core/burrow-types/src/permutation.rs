//! A page order, validated on the way in.

use crate::{Error, Result};

/// A new order for a document's pages, as zero-based indices into the old order.
///
/// `[2, 0, 1]` means "the document's third page first, then its first, then its second".
///
/// # Why a type rather than a `Vec<u64>`
///
/// `docs/ROADMAP.md` states reorder's invariant as "output is a permutation of the input — no
/// page lost, added, or duplicated". That is a property of the *request*, not of the engine's
/// answer, and checking it here means it is checked once rather than at every layer that
/// forwards a list of numbers.
///
/// A bare `Vec<u64>` can express "page 4 twice and never page 2", which produces a document
/// that is not a permutation of anything — and the failure is silent, because a document with
/// the right number of pages and the wrong pages in it looks exactly like a success. So the
/// only way to obtain one of these is [`Permutation::of`], which refuses.
///
/// # The identity is a legitimate request
///
/// `[0, 1, 2]` on a three-page document is the identity, and the ROADMAP names it: "the
/// identity permutation is a no-op". It is accepted rather than refused, because it is what a
/// person dragging a page and putting it back has asked for, and because a property test needs
/// it. What the *operation* does with it — whether it writes the document unchanged or skips
/// the work — is the operation's business, not this type's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permutation {
    /// Zero-based source indices, in the order the pages should end up.
    order: Vec<u64>,
}

impl Permutation {
    /// Build a permutation of `pages` pages from `order`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidArgument`] if `order` is not a permutation of `0..pages`: the wrong
    /// length, an index past the end, or an index used twice. The message says which of the
    /// three it was, because "invalid page order" is a refusal a caller cannot act on.
    ///
    /// Nothing here is derived from a document — `pages` is a count the engine reported and
    /// `order` is the caller's request — so the messages may name numbers.
    pub fn of(order: Vec<u64>, pages: u64) -> Result<Self> {
        let length = u64::try_from(order.len())
            .map_err(|_| Error::Internal("page order length does not fit in u64".to_owned()))?;

        if length != pages {
            return Err(Error::InvalidArgument(format!(
                "a page order must name every page exactly once: {pages} pages, {length} given"
            )));
        }

        // SEEN-BEFORE RATHER THAN SORT-AND-COMPARE. Sorting a copy and comparing it against
        // `0..pages` is the obvious check and allocates a second list as long as the document;
        // this is one BYTE per page -- `Vec<bool>` is not a bitset, and the first version of
        // this comment said "one bit" and claimed 1.25 KB where the real figure is 10 KB. On a
        // 10,000-page document that is 80 KB against 10 KB, which is still the right trade and
        // is now the number the code actually produces. Found by code review; `CLAUDE.md` puts
        // an overclaiming comment in the same class as a bug.
        let capacity = usize::try_from(pages)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let mut seen = vec![false; capacity];

        for &index in &order {
            let at = usize::try_from(index).map_err(|_| {
                Error::InvalidArgument(format!("page {index} is not in the document"))
            })?;
            let Some(slot) = seen.get_mut(at) else {
                // `pages` is the length, and indices are zero-based, so `pages` itself is one
                // past the end. Reported with the one-based number a person would use.
                //
                // `saturating_add`, NOT `+ 1`: on a 64-bit target `usize::try_from(u64::MAX)`
                // succeeds, so the biggest index a caller can send reaches this line and
                // `index + 1` panics in debug and wraps in release -- a panic in library code,
                // which `core/CLAUDE.md` forbids outright. Found by this module's own
                // `a_huge_index_is_refused_rather_than_truncated` test, which was written for
                // the `usize` conversion above and caught this instead.
                return Err(Error::InvalidArgument(format!(
                    "page {} is not in the document, which has {pages}",
                    index.saturating_add(1)
                )));
            };
            if *slot {
                return Err(Error::InvalidArgument(format!(
                    "page {} appears twice in the order",
                    index.saturating_add(1)
                )));
            }
            *slot = true;
        }

        // Every index was in range and none repeated, and there are exactly `pages` of them --
        // so every page is named exactly once. Stated rather than re-checked: a second loop
        // over `seen` would be the same conclusion reached twice.
        Ok(Self { order })
    }

    /// The source indices, in output order.
    #[must_use]
    pub fn order(&self) -> &[u64] {
        &self.order
    }

    /// How many pages this reorders.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether it reorders nothing. Only a zero-page document, which no caller can produce.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Whether this leaves every page where it is.
    ///
    /// The ROADMAP's second invariant for the operation. Offered so the operation and its
    /// tests agree on what "the identity" means rather than each deciding.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.order
            .iter()
            .enumerate()
            .all(|(at, &index)| u64::try_from(at).is_ok_and(|at| at == index))
    }
}

#[cfg(test)]
mod tests {
    use super::Permutation;
    use crate::Error;

    #[test]
    fn an_ordinary_permutation_is_accepted() {
        let p = Permutation::of(vec![2, 0, 1], 3).unwrap();
        assert_eq!(p.order(), &[2, 0, 1]);
        assert_eq!(p.len(), 3);
    }

    #[test]
    fn the_identity_is_accepted_and_says_so() {
        // The ROADMAP names it: "the identity permutation is a no-op". Refusing it would make
        // "drag a page and put it back" an error.
        let p = Permutation::of(vec![0, 1, 2, 3], 4).unwrap();
        assert!(p.is_identity());
    }

    #[test]
    fn a_reversal_is_not_the_identity() {
        assert!(!Permutation::of(vec![1, 0], 2).unwrap().is_identity());
        // A single page is always the identity, and is worth pinning: `all` over an empty-ish
        // list is the shape that silently returns true.
        assert!(Permutation::of(vec![0], 1).unwrap().is_identity());
    }

    #[test]
    fn a_short_order_is_refused_with_both_numbers() {
        // The most likely mistake, and the one a bare "invalid" would make unactionable: a
        // caller that built its list from a filtered view of the pages.
        let refused = Permutation::of(vec![0, 1], 3);
        match refused {
            Err(Error::InvalidArgument(message)) => {
                assert!(message.contains('3'), "{message}");
                assert!(message.contains('2'), "{message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_long_order_is_refused_too() {
        assert!(matches!(
            Permutation::of(vec![0, 1, 2], 2),
            Err(Error::InvalidArgument(_))
        ));
    }

    #[test]
    fn a_page_past_the_end_is_refused() {
        // Indices are zero-based, so `pages` itself is one past the end -- the boundary this
        // is most likely to get wrong.
        // 2 is the last page of three, so this side of the boundary is accepted.
        assert!(Permutation::of(vec![0, 1, 2], 3).is_ok());
        assert!(matches!(
            Permutation::of(vec![0, 1, 3], 3),
            Err(Error::InvalidArgument(_))
        ));
    }

    #[test]
    fn a_repeated_page_is_refused_rather_than_duplicated() {
        // THE INVARIANT THAT MATTERS MOST. `[0, 0, 2]` has the right length and every index in
        // range, and would produce a three-page document with page 2 missing and page 1 twice
        // -- which looks exactly like a success.
        match Permutation::of(vec![0, 0, 2], 3) {
            Err(Error::InvalidArgument(message)) => assert!(message.contains("twice"), "{message}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_huge_index_is_refused_rather_than_truncated() {
        // `u64::MAX` does not fit in a `usize` on a 32-bit target, and `as` would wrap it to
        // something in range. This crate denies casts for exactly that reason.
        assert!(matches!(
            Permutation::of(vec![u64::MAX, 1], 2),
            Err(Error::InvalidArgument(_))
        ));
    }

    #[test]
    fn a_zero_page_document_takes_only_an_empty_order() {
        // Unreachable through any operation -- qpdf refuses to open a document with no pages --
        // and this is a public constructor that does not get to assume its caller.
        assert!(Permutation::of(vec![], 0).unwrap().is_empty());
        assert!(matches!(
            Permutation::of(vec![0], 0),
            Err(Error::InvalidArgument(_))
        ));
    }
}
