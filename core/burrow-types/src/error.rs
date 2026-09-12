//! The error taxonomy every burrow operation reports through.

/// A `Result` whose error is always a burrow [`Error`].
pub type Result<T> = core::result::Result<T, Error>;

/// Everything that can go wrong in a burrow operation.
///
/// Variants are stable enough to match on across the FFI boundary, but the enum is
/// `#[non_exhaustive]`: adding a variant is not a breaking change, so callers must keep
/// a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The input is not the format it claimed to be, or is damaged beyond repair.
    #[error("malformed input: {0}")]
    Malformed(String),

    /// The input is a recognised format that burrow does not support yet.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// The input is encrypted and no usable password was supplied.
    #[error("input is password protected")]
    PasswordRequired,

    /// A [`Limits`](crate::Limits) ceiling was reached. Never a crash — always this.
    ///
    /// `limit` and `stage` answer different questions and both are needed. `limit` is the
    /// field the **caller set**, so they can see which number to change. `stage` is the
    /// **check that fired**, and for `max_memory_bytes` that is three different mechanisms
    /// with three different meanings — see [`Stage`](crate::Stage).
    #[error("limit exceeded: {limit} at {stage} (requested {requested}, allowed {allowed})")]
    LimitExceeded {
        /// Which ceiling was hit, e.g. `"max_pages"`. The field the caller set.
        limit: &'static str,
        /// Which check rejected the operation. The route it was reached by.
        stage: crate::Stage,
        /// What the input asked for.
        requested: u64,
        /// What the configured limit permitted.
        allowed: u64,
    },

    /// The caller passed arguments that cannot describe a valid operation.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// An I/O error. Never carries user file *content*, only the failure.
    #[error("io error: {0}")]
    Io(String),

    /// A bug in burrow, or a panic caught at an FFI boundary. Should never be seen.
    #[error("internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_exceeded_names_the_limit_and_both_numbers() {
        let err = Error::LimitExceeded {
            limit: "max_pages",
            stage: crate::Stage::PageCount,
            requested: 90_000,
            allowed: 10_000,
        };
        let rendered = err.to_string();
        assert!(rendered.contains("max_pages"), "{rendered}");
        assert!(rendered.contains("90000"), "{rendered}");
        assert!(rendered.contains("10000"), "{rendered}");
        // The route is in the rendered text too. A limit failure a user reports should say
        // which check produced it without needing the caller to have logged it separately.
        assert!(rendered.contains("page_count"), "{rendered}");
    }

    #[test]
    fn errors_are_send_and_sync_so_they_survive_thread_and_ffi_boundaries() {
        fn assert_send_sync<T: Send + Sync + 'static>() {}
        assert_send_sync::<Error>();
    }
}
