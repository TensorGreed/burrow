//! A document password, carried as bytes and never rendered.

use core::fmt;

use zeroize::Zeroize;

/// A password for an encrypted document.
///
/// Three properties, each of which exists because getting it wrong leaks a user's secret:
///
/// - **Bytes, not `String`.** PDF passwords are byte strings; the format does not promise
///   UTF-8, and a lossy conversion would silently change the password.
/// - **It never renders.** [`Debug`] prints a fixed placeholder and there is no
///   [`Display`](fmt::Display), so a password cannot reach a log, an error message, or a
///   panic payload by accident.
/// - **It is zeroed on drop.** The buffer is wiped with a write the compiler may not
///   elide, so it does not linger in freed memory.
///
/// The bytes the caller passed to [`Password::new`] are *copied*; wiping the caller's own
/// copy is the caller's job.
pub struct Password(Box<[u8]>);

impl Password {
    /// Takes a copy of `bytes` as a password.
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        Self(bytes.to_vec().into_boxed_slice())
    }

    /// The password bytes.
    ///
    /// Named to be conspicuous at the call site: everything this type protects against is
    /// undone by putting the result somewhere it can be seen. Engine code needs it to
    /// build the argument the C API takes, and nothing else should call it.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the password is empty.
    ///
    /// An empty password is not the same as no password: PDF's standard security handler
    /// treats the empty string as a real user password, and many encrypted documents open
    /// with it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Overwrite the buffer with zeros.
    ///
    /// Factored out of [`Drop`] so a test can observe the wipe on a live value. Reading
    /// the buffer *after* the drop would be a use-after-free, and this crate forbids the
    /// `unsafe` that would take.
    fn wipe(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Password {
    /// Always the same string, whatever the password is.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        self.wipe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_renders_the_password() {
        let password = Password::new(b"hunter2");
        let rendered = format!("{password:?}");
        assert_eq!(rendered, "Password(<redacted>)");
        assert!(!rendered.contains("hunter2"), "{rendered}");
    }

    #[test]
    fn debug_of_a_containing_struct_does_not_leak_it_either() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Options {
            password: Option<Password>,
        }
        let rendered = format!(
            "{:?}",
            Options {
                password: Some(Password::new(b"s3cret")),
            }
        );
        assert!(!rendered.contains("s3cret"), "{rendered}");
    }

    #[test]
    fn bytes_round_trip_including_non_utf8_and_interior_nul() {
        // A PDF password is a byte string. Both of these are legal and neither survives a
        // trip through `String`.
        for raw in [b"\xff\xfe\x00\x01".as_slice(), b"a\0b".as_slice()] {
            assert_eq!(Password::new(raw).as_bytes(), raw);
        }
    }

    #[test]
    fn an_empty_password_is_distinct_from_no_password() {
        let empty = Password::new(b"");
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.as_bytes(), b"");
    }

    #[test]
    fn wiping_zeroes_the_buffer_and_is_what_drop_runs() {
        let mut password = Password::new(b"hunter2");
        assert_eq!(password.as_bytes(), b"hunter2");
        password.wipe();
        assert_eq!(
            password.as_bytes(),
            &[0u8; 7],
            "the password survived its wipe"
        );
        // `Drop::drop` calls exactly this; observing it after the drop would be a
        // use-after-free, so this is the honest half of the assertion. The other half is
        // that `Drop` has one line and it is `self.wipe()`.
    }
}
