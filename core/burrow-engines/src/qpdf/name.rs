//! A PDF name, in the one form qpdf's C API accepts.
//!
//! # The bug this type exists to make unwritable
//!
//! `qpdf_oh_get_key` wants the key **with its leading slash**: `"/Parent"`, not `"Parent"`.
//! Given the wrong spelling it does not raise, does not warn, and does not return an error —
//! it returns a **null object**, exactly as it does for a key that is genuinely absent.
//!
//! Measured, on the first run of `sharing.rs`: every lookup in the resource walk returned null,
//! so the walk reported a document that draws nothing. For a *sharing* count that is the
//! dangerous direction — nothing drawn reads as **unshared**, and unshared means edit the form
//! in place, which removes its text from every other page that draws it. A silent wrong answer
//! that turns into a damaged document two steps later.
//!
//! `rotate.rs` had it right from the start (`b"/Rotate\0"`). Nothing made the next caller do
//! the same, which is the definition of a rule that is not a control.
//!
//! # What this type enforces, and where
//!
//! [`ObjectHandle`](super::handle::ObjectHandle)'s key methods take a `&Name` rather than a
//! `*const c_char`, so a raw string cannot reach the FFI at all. A `Name` comes from one of
//! exactly two places:
//!
//! - [`Name::literal`], a `const fn` whose `assert!` fails **at compile time** in a `const`
//!   item. `const PARENT: Name = Name::literal(b"Parent\0")` does not build.
//! - [`Name::from_stripped`], for names read out of a document, which puts the slash on and
//!   refuses an embedded NUL rather than letting a C string end early and act on a shorter key.

use core::ffi::c_char;

use burrow_types::{Error, Result};

/// A NUL-terminated PDF name with its leading `/`.
///
/// Borrowed for the compile-time case so a constant costs no allocation; owned for names built
/// from a document's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Name {
    /// A name written in this source, checked when it was written.
    Literal(&'static [u8]),
    /// A name read out of a document, checked when it was built.
    Read(Vec<u8>),
}

impl Name {
    /// A name written in source, checked at compile time.
    ///
    /// # Panics
    ///
    /// In a `const` item this is a **compile error**, which is the point: a missing `/` or a
    /// missing NUL cannot reach a running program. The slice pattern rather than indexing
    /// because `indexing_slicing` is denied in this crate and a bounds check is not the
    /// interesting part.
    pub(super) const fn literal(bytes: &'static [u8]) -> Self {
        assert!(
            matches!(bytes, [b'/', _, .., 0]),
            "a PDF name for qpdf must begin with '/' and end with a NUL, as b\"/Parent\\0\""
        );
        Self::Literal(bytes)
    }

    /// A name read out of a document, whose leading `/` has been stripped.
    ///
    /// `pdfsyntax::dict::top_level_keys` returns keys without the slash, and qpdf requires it.
    /// This is the one place that conversion happens.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] if the key contains a NUL. A C string would end there, so the call
    /// would silently act on a **different, shorter key** — the same class of silent wrong
    /// answer this whole type is about, arriving from the document instead of from the source.
    pub(super) fn from_stripped(key: &[u8]) -> Result<Self> {
        if key.contains(&0) {
            return Err(Error::Malformed(
                "pdf name [embedded-nul]: a dictionary key containing a NUL, which a C string \
                 would end at -- acting on a shorter key than the document names"
                    .to_owned(),
            ));
        }
        let mut owned = Vec::with_capacity(key.len() + 2);
        owned.push(b'/');
        owned.extend_from_slice(key);
        owned.push(0);
        Ok(Self::Read(owned))
    }

    /// A name that **already** carries its leading `/`, from a caller that canonicalised it.
    ///
    /// `prune`'s policy hands over names in this form. Separate from [`Self::from_stripped`]
    /// rather than sniffing for the slash, because a name that may or may not have one is
    /// exactly the ambiguity this type exists to remove: `/Parent` and `Parent` would both
    /// "work", and one of them silently wrongly.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for a missing leading `/` or an embedded NUL.
    pub(super) fn from_canonical(key: &[u8]) -> Result<Self> {
        if !matches!(key, [b'/', ..]) {
            return Err(Error::Malformed(
                "pdf name [missing-slash]: a canonical PDF name must begin with '/'".to_owned(),
            ));
        }
        if key.contains(&0) {
            return Err(Error::Malformed(
                "pdf name [embedded-nul]: a name containing a NUL, which a C string would end                  at -- acting on a shorter name than the document gives"
                    .to_owned(),
            ));
        }
        let mut owned = Vec::with_capacity(key.len() + 1);
        owned.extend_from_slice(key);
        owned.push(0);
        Ok(Self::Read(owned))
    }

    /// The pointer qpdf wants.
    pub(super) fn as_ptr(&self) -> *const c_char {
        match self {
            Self::Literal(bytes) => bytes.as_ptr().cast(),
            Self::Read(owned) => owned.as_ptr().cast(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Name;

    /// The names this crate uses, so the `const` evaluation actually runs on each.
    const PARENT: Name = Name::literal(b"/Parent\0");
    const SHORTEST: Name = Name::literal(b"/N\0");

    #[test]
    fn a_literal_name_carries_its_slash_and_terminator() {
        assert_eq!(PARENT, Name::Literal(b"/Parent\0"));
        assert_eq!(SHORTEST, Name::Literal(b"/N\0"));
    }

    #[test]
    fn a_stripped_key_gets_its_slash_back() {
        assert_eq!(
            Name::from_stripped(b"Parent").expect("valid"),
            Name::Read(b"/Parent\0".to_vec())
        );
    }

    #[test]
    fn a_key_containing_a_nul_is_refused_rather_than_truncated() {
        // A C string ends at the NUL, so `/Sec` would be looked up instead of `/Sec\0ret` --
        // a different key, silently. The document chooses these bytes.
        let error = Name::from_stripped(b"Sec\0ret").expect_err("must refuse");
        assert!(
            format!("{error:?}").contains("embedded-nul"),
            "refused by a different rule: {error:?}"
        );
    }

    #[test]
    fn the_compile_time_check_is_real_and_not_a_comment() {
        // `Name::literal` is a `const fn` whose `assert!` fails during const evaluation, so a
        // missing slash is a BUILD failure rather than a runtime one. That cannot be asserted
        // from inside a test -- a test that compiles is a test whose constants compiled -- so
        // the evidence is `tools/test-name-requires-slash.sh`, which tries to build each
        // rejected spelling and requires the compiler to refuse it.
        //
        // What IS assertable here: the same predicate, on the same inputs, at run time.
        for bad in [
            b"Parent\0".as_slice(),
            b"/Parent".as_slice(),
            b"/\0".as_slice(),
            b"".as_slice(),
        ] {
            assert!(
                !matches!(bad, [b'/', _, .., 0]),
                "{:?} must not satisfy the name predicate",
                core::str::from_utf8(bad)
            );
        }
        for good in [b"/Parent\0".as_slice(), b"/N\0".as_slice()] {
            assert!(matches!(good, [b'/', _, .., 0]));
        }
    }
}
