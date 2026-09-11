//! Preparing a caller's password for an engine's C API.
//!
//! One copy, shared by every engine and every platform. It was two — `pdfium::password_arg`
//! and `qpdf::password_arg`, byte-identical but for the engine name in the error message —
//! and M1 PR 4's web path would have made it three. A rule with three implementations is a
//! rule that will eventually have two behaviours.
//!
//! Nothing here is platform-specific: it copies bytes, appends a NUL, and refuses input a
//! C string cannot carry. That is why it compiles everywhere rather than behind the
//! native-engine gate.

use burrow_types::{Error, Password, Result};
use zeroize::Zeroizing;

/// A NUL-terminated copy of a password, wiped when it goes out of scope.
///
/// `None` means no password was supplied, which is different from an empty one.
pub(crate) type NulTerminated = Option<Zeroizing<Vec<u8>>>;

/// Copy a password into the NUL-terminated form an engine's C API takes.
///
/// The copy exists because the C API needs a terminator the caller's [`Password`] does not
/// carry, and it is [`Zeroizing`] so the copy is wiped rather than left in the allocator's
/// free list. Callers should `drop` it as soon as the engine has read it, not at the end of
/// the enclosing scope.
///
/// `engine` names the engine in the error message and must be a fixed constant — it is
/// interpolated into a message, and `core/CLAUDE.md` forbids anything input-derived
/// reaching one.
///
/// # Errors
///
/// [`Error::InvalidArgument`] if the password contains a NUL byte. Both engines take a
/// NUL-terminated C string, so an interior NUL would silently truncate the password and the
/// document would fail to open for a reason the user could not possibly guess. Failing
/// loudly is better — and the message says only that, never the password, not even its
/// length.
pub(crate) fn nul_terminated(
    password: Option<&Password>,
    engine: &'static str,
) -> Result<NulTerminated> {
    let Some(password) = password else {
        return Ok(None);
    };
    if password.as_bytes().contains(&0) {
        return Err(Error::InvalidArgument(format!(
            "password contains a NUL byte, which {engine}'s C API cannot carry"
        )));
    }
    let mut buf = Vec::with_capacity(password.len().saturating_add(1));
    buf.extend_from_slice(password.as_bytes());
    buf.push(0);
    Ok(Some(Zeroizing::new(buf)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_password_stays_no_password() {
        assert!(
            nul_terminated(None, "pdfium")
                .expect("none is valid")
                .is_none()
        );
    }

    #[test]
    fn a_password_is_copied_and_nul_terminated() {
        let password = Password::new(b"hunter2");
        let prepared = nul_terminated(Some(&password), "qpdf")
            .expect("an ordinary password is valid")
            .expect("a password was supplied");
        assert_eq!(&prepared[..], b"hunter2\0");
    }

    /// An empty password is a real thing and is not the same as no password: a document
    /// can be encrypted with one.
    #[test]
    fn an_empty_password_is_not_the_same_as_none() {
        let password = Password::new(b"");
        let prepared = nul_terminated(Some(&password), "qpdf")
            .expect("an empty password is valid")
            .expect("a password was supplied");
        assert_eq!(&prepared[..], b"\0");
    }

    #[test]
    fn an_interior_nul_is_refused_without_echoing_the_password() {
        let password = Password::new(b"before\0after");
        let error = nul_terminated(Some(&password), "pdfium").expect_err("a NUL is refused");
        assert!(matches!(error, Error::InvalidArgument(_)), "{error:?}");
        let rendered = error.to_string();
        for leaked in ["before", "after"] {
            assert!(
                !rendered.contains(leaked),
                "{leaked:?} leaked into {rendered:?}"
            );
        }
        // Nor its length, which would narrow a guess.
        assert!(
            !rendered.chars().any(|c| c.is_ascii_digit()),
            "a digit reached {rendered:?}"
        );
    }

    #[test]
    fn the_engine_name_reaches_the_message_so_the_caller_knows_which_api_refused() {
        let password = Password::new(b"a\0b");
        for engine in ["pdfium", "qpdf"] {
            let rendered = nul_terminated(Some(&password), engine)
                .expect_err("a NUL is refused")
                .to_string();
            assert!(rendered.contains(engine), "{rendered:?}");
        }
    }
}
