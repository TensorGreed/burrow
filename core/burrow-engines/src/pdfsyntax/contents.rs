//! A page's `/Contents` array as one lexical stream, and the map back to the streams it came from.
//!
//! PDF 32000-1 §7.8.2: where `/Contents` is an array, *"the divisions between the streams occur
//! only at the boundaries between lexical tokens"* and the array as a whole **is** the page's
//! content stream. A producer may therefore end one element mid-text-object and begin the next
//! with the `ET`, and readers that tokenise each element separately see two parses, neither of
//! which holds a complete text object.
//!
//! That is not hypothetical. Spike 0006's channel 22 (`22-split-content-streams.pdf`) is exactly
//! this document, and the spike measured the naive route — read each element, rewrite it, write it
//! back — silently collapsing the array into one stream.
//!
//! # Why `qpdf_oh_get_page_content_data` is not enough on its own
//!
//! That entry point already hands back the **concatenation**, which is the right thing to read.
//! What it does not hand back is which byte came from which element, so a rewriter holding only
//! its output can write the page back as one stream and nothing else. Keeping the array is not
//! cosmetic: an element may be shared with another page, and a page whose `/Contents` was an array
//! of two and is now one stream has had its object graph rewritten by an operation that was asked
//! to remove some text.
//!
//! So this concatenates the elements **itself**, keeping the boundaries, and cuts the rewritten
//! bytes back along them. ADR 0029 §4.
//!
//! # The separator is a byte of real content, not a formatting choice
//!
//! Elements are joined with a single line feed. Without one, an element ending `BT` and the next
//! beginning `/F1` would concatenate to `BT/F1`, which still lexes — but an element ending `72` and
//! the next beginning `720 Td` concatenates to `72720 Td`, which is one number where there were
//! two. Every reader inserts white space here for that reason. The separator belongs to no element,
//! so an edit may not touch it and no element gets it back on the way out.

use burrow_types::{Error, Result};

use super::ops::Span;

/// The byte inserted between two `/Contents` elements. See the module header.
const SEPARATOR: u8 = b'\n';

/// The most elements one page's `/Contents` array may have.
///
/// A real page has one or two; a producer that writes one per operator is unusual and still well
/// under this. It is a bound on work as much as on memory: every element is a stream object qpdf
/// must fetch and decode, and `apply` walks them. The security review of #128 measured 64,000
/// elements at 3.44 s in `apply` alone, before the binary search in [`Contents::locate`] and the
/// merge walk in [`Contents::apply`] replaced the two linear scans.
pub const MAX_ELEMENTS: usize = 4_096;

/// A page's content streams concatenated, with the map back to each one.
#[derive(Debug, Clone)]
pub struct Contents {
    joined: Vec<u8>,
    /// Each element's range in `joined`, in order, excluding the separator after it.
    parts: Vec<Span>,
}

/// One replacement to make in the concatenation.
///
/// The rewriter produces these from [`Operation`](super::ops::Operation) spans; `apply` is what
/// turns them back into per-stream bytes.
///
/// **Not `#[non_exhaustive]`, unlike [`Operand`](super::ops::Operand) and
/// [`Operation`](super::ops::Operation).** Those two are answers this module hands out and
/// nobody else constructs, so sealing them costs nothing. This one is an *instruction* a caller
/// writes — `burrow-ops`' redaction is the caller, and #131 is where it lands — and a sealed
/// struct cannot be constructed outside the crate that defines it. The cost is that adding a
/// field here is a breaking change, which is the right way round for a two-field instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// The bytes to replace, as a range into [`Contents::bytes`].
    pub span: Span,
    /// What to put there. Empty removes the range.
    pub replacement: Vec<u8>,
}

impl Contents {
    /// Concatenate `parts` in order, keeping the map back to each.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] if `parts` is empty — a page whose `/Contents` array has no elements
    /// has no content stream, and a caller that meant "one empty stream" should say so with one
    /// empty element.
    pub fn concatenate(parts: &[&[u8]]) -> Result<Self> {
        if parts.is_empty() {
            return Err(Error::Malformed(
                "pdf syntax: a /Contents array with no elements".to_owned(),
            ));
        }
        if parts.len() > MAX_ELEMENTS {
            return Err(Error::Unsupported(
                "a page's /Contents array holds more streams than burrow will read".to_owned(),
            ));
        }
        let total: usize = parts.iter().map(|p| p.len() + 1).sum();
        let mut joined = Vec::with_capacity(total);
        let mut spans = Vec::with_capacity(parts.len());
        for part in parts {
            let start = joined.len();
            joined.extend_from_slice(part);
            spans.push((start, joined.len()));
            joined.push(SEPARATOR);
        }
        Ok(Self {
            joined,
            parts: spans,
        })
    }

    /// The bytes to tokenise: every element, in order, separated.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.joined
    }

    /// How many elements the page's `/Contents` had.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    /// Never true — [`concatenate`](Self::concatenate) refuses an empty array — and present
    /// because `len` without `is_empty` is a clippy lint and a reader's question.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    /// Which element `offset` came from, and where in it, or `None` for a separator or past the end.
    ///
    /// This is the map the module exists for. A rewriter reports what it changed in terms of the
    /// concatenation, because that is what it tokenised; anything that has to name a *stream* —
    /// a diagnostic, a disclosure, a test asserting which element was touched — comes through here.
    #[must_use]
    pub fn locate(&self, offset: usize) -> Option<(usize, usize)> {
        // BINARY, not linear. `parts` is sorted and disjoint by construction, and the linear
        // scan this replaced made `apply` quadratic in elements x edits: the security review of
        // #128 measured a 320 kB page with 64,000 elements costing 3.44 s inside one call, with
        // no cooperative deadline check anywhere in it. Both factors are attacker-controlled.
        let after = self.parts.partition_point(|&(start, _)| start <= offset);
        let index = after.checked_sub(1)?;
        let &(start, end) = self.parts.get(index)?;
        (offset >= start && offset < end).then_some((index, offset - start))
    }

    /// Which element `offset` belongs to, or `None` for a separator or past the end.
    #[must_use]
    fn element_of(&self, offset: usize) -> Option<usize> {
        self.locate(offset).map(|(index, _)| index)
    }

    /// Apply `edits` and cut the result back into one buffer per original element.
    ///
    /// The returned vector has exactly [`len`](Self::len) entries, in the original order, so a
    /// caller can write each one back to the stream it came from and leave the `/Contents` array
    /// the shape it found it.
    ///
    /// # An edit that crosses an element boundary may only delete
    ///
    /// Because the divisions fall between tokens and nowhere else, an operation *can* straddle
    /// two elements — `72 7` in one and `20 Td` in the next is a legal way to write `72 720 Td`.
    /// Deleting across the boundary is well defined: each element loses the bytes it held, and no
    /// byte has to be given an owner. **Replacing** across it is not, and this refuses it.
    ///
    /// The refusal is not fastidiousness. The first version of this put the whole replacement in
    /// the element the edit started in and deleted the rest, which reads as harmless because the
    /// concatenation is identical — and `pdfsyntax_contents`' no-op property caught it in under a
    /// minute: replacing every operation with **its own bytes** moved content out of the later
    /// element into the earlier one. A page whose second content stream is shared with another
    /// page would have had that page's bytes rewritten by an operation asked to change nothing.
    /// There is no rule for which element a replacement byte belongs to, so the caller is told
    /// rather than guessed at; ADR 0029 §4, and #131 decides what the operation does about it.
    ///
    /// # An edit that changes nothing is refused
    ///
    /// The separators belong to no element, so an edit whose span covers only separator bytes
    /// deletes nothing — and the first version returned `Ok` for it. **18 such edits** exist
    /// across the single-edit combinations of six small element shapes; the security review of
    /// #128 enumerated them. No content byte is involved, so it is not a leak today. It is a
    /// leak *shape*: `apply` says nothing about which edits landed, so nothing downstream can
    /// tell "applied" from "accepted and discarded", and `CLAUDE.md`'s control 5 is that a
    /// removal nothing observed is not a measured removal. So it is refused at the door.
    ///
    /// An **insertion** — an empty span with a replacement — is placed in the element whose
    /// range contains the offset, and an offset at an element's end belongs to that element.
    /// That is what makes appending to an element possible; the first version refused it,
    /// because the byte at that offset is a separator.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`] — an edit is out of order, overlaps another, reaches past the end,
    ///   changes nothing, or replaces (rather than deletes) across an element boundary.
    pub fn apply(&self, edits: &[Edit]) -> Result<Vec<Vec<u8>>> {
        let mut previous_end = 0usize;
        for edit in edits {
            let (start, end) = edit.span;
            if start < previous_end || end < start || end > self.joined.len() {
                return Err(Error::Malformed(
                    "pdf syntax: a content-stream edit that overlaps another, runs backwards, \
                     or reaches past the stream"
                        .to_owned(),
                ));
            }
            previous_end = end;

            let inserting = start == end;
            if inserting && edit.replacement.is_empty() {
                return Err(Error::Malformed(
                    "pdf syntax: a content-stream edit that neither removes nor writes anything"
                        .to_owned(),
                ));
            }
            if inserting {
                if self.element_for_insertion(start).is_none() {
                    return Err(Error::Malformed(
                        "pdf syntax: a content-stream insertion at an offset that belongs to no \
                         /Contents element"
                            .to_owned(),
                    ));
                }
                continue;
            }

            // A deletion or replacement. It has to cover at least one byte some element owns,
            // or it is a removal of nothing dressed as a removal.
            let opens = self.element_of(start);
            // `end` is exclusive, so the last byte covered is `end - 1`.
            let closes = end.checked_sub(1).and_then(|last| self.element_of(last));
            if opens.is_none() && closes.is_none() {
                return Err(Error::Malformed(
                    "pdf syntax: a content-stream edit covering only the bytes between two \
                     /Contents elements, which belong to neither and would remove nothing"
                        .to_owned(),
                ));
            }
            if !edit.replacement.is_empty() && (opens.is_none() || opens != closes) {
                return Err(Error::Malformed(
                    "pdf syntax: a content-stream edit that would write across the boundary \
                     between two /Contents elements. Deleting across it is defined; replacing \
                     is not, because nothing says which element the new bytes belong to"
                        .to_owned(),
                ));
            }
        }

        let mut out: Vec<Vec<u8>> = Vec::with_capacity(self.parts.len());
        let mut edit_at = 0usize;
        for &(start, end) in &self.parts {
            let mut target = Vec::new();
            let mut at = start;
            while let Some(edit) = edits.get(edit_at) {
                let (edit_start, edit_end) = edit.span;
                // An insertion at exactly this element's end belongs to this element; anything
                // else starting at or past `end` belongs to a later one.
                let mine = edit_start < end || (edit_start == edit_end && edit_start == end);
                if !mine {
                    break;
                }
                if edit_end > at || edit_start >= at {
                    let keep = self
                        .joined
                        .get(at..edit_start.clamp(at, end))
                        .ok_or_else(|| {
                            Error::Internal("pdf syntax: a kept run left the stream".to_owned())
                        })?;
                    target.extend_from_slice(keep);
                }
                // The replacement is emitted by the element the edit OPENS in; validation has
                // already refused a non-empty one that does not close there too.
                if edit_start >= start {
                    target.extend_from_slice(&edit.replacement);
                }
                at = at.max(edit_end.min(end));
                if edit_end > end {
                    // It reaches into the next element, which needs to see it as well. Left for
                    // that element rather than consumed here.
                    break;
                }
                edit_at += 1;
            }
            if at < end {
                let tail = self.joined.get(at..end).ok_or_else(|| {
                    Error::Internal("pdf syntax: a tail run left the stream".to_owned())
                })?;
                target.extend_from_slice(tail);
            }
            out.push(target);
        }

        if edit_at == edits.len() {
            Ok(out)
        } else {
            // Unreachable: validation refuses every edit that no element can claim. A refusal
            // rather than a silent drop if it ever became reachable -- an edit nobody applied
            // and nobody reported is the shape this whole milestone is about.
            Err(Error::Internal(
                "pdf syntax: a content-stream edit that no /Contents element applied".to_owned(),
            ))
        }
    }

    /// Which element an insertion at `offset` belongs to, counting an element's end as its own.
    fn element_for_insertion(&self, offset: usize) -> Option<usize> {
        let after = self.parts.partition_point(|&(start, _)| start <= offset);
        let index = after.checked_sub(1)?;
        let &(start, end) = self.parts.get(index)?;
        (offset >= start && offset <= end).then_some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::{Contents, Edit};

    fn joined(parts: &[&[u8]]) -> Contents {
        Contents::concatenate(parts).expect("concatenates")
    }

    #[test]
    fn a_text_object_split_across_two_elements_tokenises_as_one() {
        // CHANNEL 22, and the reason this module exists. Read element-wise, the first parse has
        // a `BT` with no `ET` and the second an `ET` with no `BT`.
        let contents = joined(&[b"BT /F1 12 Tf (secret) Tj", b"ET Q"]);
        let read = super::super::ops::operations(contents.bytes()).expect("reads");
        let operators: Vec<&[u8]> = read.iter().map(|o| o.operator.as_slice()).collect();
        assert_eq!(operators, [&b"BT"[..], b"Tf", b"Tj", b"ET", b"Q"]);
    }

    #[test]
    fn the_separator_stops_two_numbers_becoming_one() {
        // Without it `72` and `720 Td` concatenate to `72720 Td`: one number where there were
        // two, and a page translated ten times too far.
        let contents = joined(&[b"72", b"720 Td"]);
        let read = super::super::ops::operations(contents.bytes()).expect("reads");
        assert_eq!(read.len(), 1);
        let values: Vec<f64> = read[0]
            .operands
            .iter()
            .filter_map(super::super::ops::Operand::as_number)
            .collect();
        assert_eq!(values, [72.0, 720.0]);
    }

    #[test]
    fn every_offset_maps_back_to_the_element_it_came_from() {
        let contents = joined(&[b"abc", b"de"]);
        assert_eq!(contents.bytes(), b"abc\nde\n");
        assert_eq!(contents.locate(0), Some((0, 0)));
        assert_eq!(contents.locate(2), Some((0, 2)));
        // The separator belongs to no element.
        assert_eq!(contents.locate(3), None);
        assert_eq!(contents.locate(4), Some((1, 0)));
        assert_eq!(contents.locate(6), None);
        assert_eq!(contents.locate(999), None);
    }

    #[test]
    fn applying_no_edits_returns_each_element_exactly_as_it_was() {
        // The identity, which is the property a round-trip test rests on: a page that nothing
        // was removed from must come back byte-identical, or every later assertion is measuring
        // this function's noise.
        let parts: &[&[u8]] = &[b"BT /F1 12 Tf (a) Tj", b"ET", b""];
        let contents = joined(parts);
        let out = contents.apply(&[]).expect("applies");
        assert_eq!(out.len(), 3);
        for (got, want) in out.iter().zip(parts) {
            assert_eq!(got.as_slice(), *want);
        }
    }

    #[test]
    fn an_edit_inside_one_element_leaves_the_others_untouched() {
        let contents = joined(&[b"BT (secret) Tj ET", b"Q"]);
        let start = 3;
        let end = start + b"(secret) Tj".len();
        let out = contents
            .apply(&[Edit {
                span: (start, end),
                replacement: Vec::new(),
            }])
            .expect("applies");
        assert_eq!(out[0], b"BT  ET");
        assert_eq!(out[1], b"Q");
    }

    #[test]
    fn replacing_across_an_element_boundary_is_refused_rather_than_quietly_moving_content() {
        // `72 7` + `20 Td` is a legal way to write `72 720 Td`, so the operation straddles. An
        // earlier version put the whole replacement in the first element; the concatenation was
        // identical and the SECOND ELEMENT had lost bytes it owned. `pdfsyntax_contents` found
        // it with the no-op property below.
        let contents = joined(&[b"q 72 7", b"20 Td Q"]);
        let end = contents
            .bytes()
            .windows(2)
            .position(|w| w == b"Td")
            .expect("finds Td")
            + 2;
        let refused = contents.apply(&[Edit {
            span: (2, end),
            replacement: b"0 0 Td".to_vec(),
        }]);
        assert!(
            refused.is_err(),
            "a cross-boundary replacement must be refused"
        );
    }

    #[test]
    fn deleting_across_an_element_boundary_is_defined_and_allowed() {
        // Deleting needs no rule for who owns a byte: each element loses what it held. This is
        // the case redaction actually produces, which is why the refusal above is narrow.
        let contents = joined(&[b"q 72 7", b"20 Td Q"]);
        let end = contents
            .bytes()
            .windows(2)
            .position(|w| w == b"Td")
            .expect("finds Td")
            + 2;
        let out = contents
            .apply(&[Edit {
                span: (2, end),
                replacement: Vec::new(),
            }])
            .expect("applies");
        assert_eq!(out[0], b"q ");
        assert_eq!(out[1], b" Q");
    }

    #[test]
    fn replacing_every_operation_with_its_own_bytes_returns_the_input() {
        // THE NO-OP PROPERTY, in a unit test as well as in the fuzz target, because it is the
        // one that found the defect above and a fuzz finding nobody pinned is a finding that
        // comes back.
        let parts: &[&[u8]] = &[b"q 1 0 0 1 72 720 cm", b"BT (a) Tj ET Q"];
        let contents = joined(parts);
        let bytes = contents.bytes().to_vec();
        let edits: Vec<Edit> = super::super::ops::operations(&bytes)
            .expect("reads")
            .iter()
            .map(|operation| Edit {
                span: operation.span,
                replacement: bytes[operation.span.0..operation.span.1].to_vec(),
            })
            .collect();
        let out = contents.apply(&edits).expect("applies");
        for (got, want) in out.iter().zip(parts) {
            assert_eq!(got.as_slice(), *want);
        }
    }

    #[test]
    fn edits_out_of_order_or_overlapping_are_refused_rather_than_silently_reordered() {
        let contents = joined(&[b"0123456789"]);
        let backwards = [
            Edit {
                span: (5, 7),
                replacement: Vec::new(),
            },
            Edit {
                span: (1, 3),
                replacement: Vec::new(),
            },
        ];
        assert!(contents.apply(&backwards).is_err());
        let overlapping = [
            Edit {
                span: (1, 5),
                replacement: Vec::new(),
            },
            Edit {
                span: (3, 7),
                replacement: Vec::new(),
            },
        ];
        assert!(contents.apply(&overlapping).is_err());
    }

    #[test]
    fn an_edit_past_the_end_is_refused_rather_than_dropped() {
        // A DROPPED EDIT IS THE FAILURE THIS WHOLE MILESTONE IS ABOUT: the operation would
        // report a removal it did not make, and the output would look redacted.
        let contents = joined(&[b"0123"]);
        let past = [Edit {
            span: (10, 12),
            replacement: Vec::new(),
        }];
        assert!(contents.apply(&past).is_err());
    }

    #[test]
    fn an_edit_that_changes_nothing_is_refused_rather_than_accepted_and_discarded() {
        // Separators belong to no element, so deleting one deletes nothing. `apply` says
        // nothing about which edits landed, so an accepted no-op is indistinguishable
        // downstream from an applied one -- control 5, one level down.
        let contents = joined(&[b"abc", b"de"]);
        assert_eq!(contents.bytes(), b"abc\nde\n");
        for span in [(3, 4), (6, 7)] {
            assert!(
                contents
                    .apply(&[Edit {
                        span,
                        replacement: Vec::new(),
                    }])
                    .is_err(),
                "an edit covering only separator bytes must be refused: {span:?}"
            );
        }
        // An empty span with nothing to write is likewise not an edit.
        assert!(
            contents
                .apply(&[Edit {
                    span: (1, 1),
                    replacement: Vec::new(),
                }])
                .is_err()
        );
    }

    #[test]
    fn an_insertion_at_an_elements_end_appends_to_that_element() {
        // The byte at that offset is a separator, so the first version refused it -- and
        // appending to an element is a thing a rewriter will want.
        let contents = joined(&[b"abc", b"de"]);
        let out = contents
            .apply(&[Edit {
                span: (3, 3),
                replacement: b" Q".to_vec(),
            }])
            .expect("appends");
        assert_eq!(out[0], b"abc Q");
        assert_eq!(out[1], b"de");
    }

    #[test]
    fn an_insertion_inside_an_element_lands_where_it_was_asked_to() {
        let contents = joined(&[b"abde"]);
        let out = contents
            .apply(&[Edit {
                span: (2, 2),
                replacement: b"c".to_vec(),
            }])
            .expect("inserts");
        assert_eq!(out[0], b"abcde");
    }

    #[test]
    fn more_elements_than_the_cap_is_a_refusal() {
        let one: &[u8] = b"q";
        let too_many = vec![one; super::MAX_ELEMENTS + 1];
        assert!(Contents::concatenate(&too_many).is_err());
        let at_the_cap = vec![one; super::MAX_ELEMENTS];
        assert_eq!(
            Contents::concatenate(&at_the_cap)
                .expect("at the cap")
                .len(),
            super::MAX_ELEMENTS
        );
    }

    #[test]
    fn an_empty_contents_array_is_refused() {
        assert!(Contents::concatenate(&[]).is_err());
    }

    #[test]
    fn one_element_is_the_ordinary_case_and_costs_only_the_separator() {
        let contents = joined(&[b"BT ET"]);
        assert_eq!(contents.len(), 1);
        assert_eq!(contents.bytes(), b"BT ET\n");
        assert_eq!(contents.apply(&[]).expect("applies")[0], b"BT ET");
    }

    #[test]
    fn apply_always_returns_one_buffer_per_element() {
        // THE INVARIANT THE OPERATION GUARDS AND CANNOT REACH. `redact::steps`' `rewrite` checks
        // that the splice returned as many buffers as the page has elements, and a mutation
        // sweep found that deleting the check changes nothing any test can see -- because
        // `apply` cannot return a different count, by construction.
        //
        // That makes the guard defence in depth over a property, and this is the property. A
        // guard whose invariant is untested is a guard nobody has checked is worth having;
        // testing it here rather than there puts it where the invariant is actually decided.
        let cases: [&[&[u8]]; 5] = [
            &[b"BT ET"],
            &[b"BT", b"ET"],
            &[b"", b"BT ET", b""],
            &[b"q", b"1 0 0 1 0 0 cm", b"Q"],
            &[b"a", b"b", b"c", b"d", b"e"],
        ];
        for parts in cases {
            let contents = joined(parts);
            for edits in [
                Vec::new(),
                vec![Edit {
                    span: (0, 1),
                    replacement: b"Z".to_vec(),
                }],
                vec![Edit {
                    span: (0, contents.bytes().len().saturating_sub(1)),
                    replacement: Vec::new(),
                }],
            ] {
                // An edit may be refused -- it can straddle a separator -- and a refusal is not
                // this property's business. What must never happen is a different count.
                if let Ok(applied) = contents.apply(&edits) {
                    assert_eq!(
                        applied.len(),
                        parts.len(),
                        "apply returned {} buffers for {} elements",
                        applied.len(),
                        parts.len()
                    );
                }
            }
        }
    }
}
