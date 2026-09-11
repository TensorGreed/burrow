//! The schema of `tests/conformance/expectations.json`.
//!
//! Included by the conformance test and by the fixture generator, so the writer and the
//! reader cannot disagree about the format.
//!
//! # Why this file is data and not a list of `assert_eq!`s
//!
//! M1 PR 4 adds a **differential conformance harness**: the same corpus run through the
//! native `DocumentEngine` and through the web one, asserting identical typed outcomes
//! (ROADMAP M1 item 12). If the expected outcomes lived in Rust `assert_eq!`s, the web
//! side would have to restate every one of them in TypeScript, and the two lists would
//! drift — which is the failure this file exists to prevent. The JSON is the contract;
//! neither implementation owns it.
//!
//! So the schema is deliberately dull and language-neutral: no Rust types leak into it,
//! the outcome is a tag rather than a message, and every field is something JavaScript
//! can read without a parser of its own.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// The whole file.
#[derive(Debug, Deserialize, Serialize)]
pub struct Expectations {
    /// Bumped when the shape below changes incompatibly. A reader that does not recognise
    /// the version must fail, not guess.
    pub schema: u32,
    /// The command that produced this file and the fixtures beside it.
    pub generated_by: String,
    /// One entry per committed fixture.
    pub cases: Vec<Case>,
}

/// One fixture and what opening it must produce.
#[derive(Debug, Deserialize, Serialize)]
pub struct Case {
    /// Short identifier, for test output.
    pub name: String,
    /// Path to the fixture, relative to the directory holding this file.
    pub file: String,
    /// sha256 of the fixture, so an edited file fails instead of quietly changing what is
    /// asserted.
    pub sha256: String,
    /// Password to open with, or `null` for none.
    ///
    /// A string, not bytes: every fixture's password is ASCII, and a JSON string is what
    /// the web harness can pass straight through. A fixture needing a non-UTF-8 password
    /// would need this field to grow a byte-array form, and that is a schema bump.
    pub password: Option<String>,
    /// What opening the fixture must produce.
    pub expect: Expect,
}

/// The required outcome: a value, or a specific error variant.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// The document opens, and reports this many pages.
    Ok {
        /// Pages the engine must report.
        page_count: u64,
    },
    /// The document does not open, and fails as this variant.
    ///
    /// The **variant**, never the message. Messages are ours to reword; a variant is the
    /// contract, and it is what both implementations have to agree on.
    Err(ErrorKind),
}

/// A `burrow_types::Error` variant, by name.
#[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
pub enum ErrorKind {
    /// [`Error::Malformed`](burrow_types::Error::Malformed).
    Malformed,
    /// [`Error::Unsupported`](burrow_types::Error::Unsupported).
    Unsupported,
    /// [`Error::PasswordRequired`](burrow_types::Error::PasswordRequired).
    PasswordRequired,
    /// [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded).
    LimitExceeded,
    /// [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument).
    InvalidArgument,
    /// [`Error::Io`](burrow_types::Error::Io).
    Io,
    /// [`Error::Internal`](burrow_types::Error::Internal).
    Internal,
}

impl ErrorKind {
    /// Classify an error by variant.
    ///
    /// Returns `None` for a variant added since this was written — `Error` is
    /// `#[non_exhaustive]`, and a case that silently matched the wildcard would pass while
    /// asserting nothing.
    pub fn of(error: &burrow_types::Error) -> Option<Self> {
        use burrow_types::Error as E;
        Some(match error {
            E::Malformed(_) => Self::Malformed,
            E::Unsupported(_) => Self::Unsupported,
            E::PasswordRequired => Self::PasswordRequired,
            E::LimitExceeded { .. } => Self::LimitExceeded,
            E::InvalidArgument(_) => Self::InvalidArgument,
            E::Io(_) => Self::Io,
            E::Internal(_) => Self::Internal,
            _ => return None,
        })
    }
}

/// Lowercase hex of a sha256 digest.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// `tests/conformance/`, found from this crate's manifest directory.
///
/// A repository-root path on purpose: the directory is shared with the web harness, so it
/// belongs to neither crate. See its `PROVENANCE.md`.
pub fn conformance_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/conformance")
        .canonicalize()
        .unwrap_or_else(|_| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/conformance")
        })
}
