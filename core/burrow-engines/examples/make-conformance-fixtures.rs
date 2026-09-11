//! Regenerate `tests/conformance/fixtures/` and `tests/conformance/expectations.json`.
//!
//! ```bash
//! cargo run -p burrow-engines --example make-conformance-fixtures
//! ```
//!
//! Every fixture is generated here rather than downloaded. That is a provenance decision,
//! not a convenience: a committed corpus file needs a licence, a source, and a
//! redistribution answer, and generated output needs none of those — it is this
//! repository's own work, under the project's own licence. It also means the expected
//! outcomes are derived from how the file was *built*, not from what an engine happened to
//! say about it.
//!
//! The expected outcomes here are written by hand, deliberately. Recording whatever the
//! engine currently returns would turn the conformance suite into a snapshot of today's
//! behaviour, and it would keep passing through the exact regression it exists to catch.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../testsupport/expectations.rs"]
mod expectations;
#[path = "../testsupport/minimal_pdf.rs"]
mod minimal_pdf;

use expectations::{Case, ErrorKind, Expect, Expectations, conformance_dir, sha256_hex};

/// One fixture to write: its case name, its filename, its bytes, the password to record,
/// and the outcome opening it must produce.
struct Fixture {
    name: &'static str,
    filename: &'static str,
    bytes: Vec<u8>,
    password: Option<&'static str>,
    expect: Expect,
}

const GENERATED_BY: &str = "cargo run -p burrow-engines --example make-conformance-fixtures";

fn main() {
    let dir = conformance_dir();
    let fixtures = dir.join("fixtures");
    std::fs::create_dir_all(&fixtures).expect("tests/conformance/fixtures should be creatable");

    let files = vec![
        Fixture {
            name: "blank-1page",
            filename: "blank-1page.pdf",
            bytes: minimal_pdf::pdf_with_pages(1),
            password: None,
            expect: Expect::Ok { page_count: 1 },
        },
        Fixture {
            name: "pages-10",
            filename: "pages-10.pdf",
            bytes: minimal_pdf::pdf_with_pages(10),
            password: None,
            expect: Expect::Ok { page_count: 10 },
        },
        Fixture {
            name: "pages-137",
            filename: "pages-137.pdf",
            // An awkward number on purpose: an off-by-one or a truncated count is visible
            // in a way it would not be for 1, 10, or a power of two.
            bytes: minimal_pdf::pdf_with_pages(137),
            password: None,
            expect: Expect::Ok { page_count: 137 },
        },
        Fixture {
            name: "truncated",
            filename: "truncated.pdf",
            bytes: minimal_pdf::truncated_pdf(),
            password: None,
            expect: Expect::Err(ErrorKind::Malformed),
        },
        Fixture {
            name: "not-a-pdf",
            filename: "not-a-pdf.bin",
            bytes: minimal_pdf::not_a_pdf(),
            password: None,
            expect: Expect::Err(ErrorKind::Malformed),
        },
        Fixture {
            name: "no-pages",
            filename: "no-pages.pdf",
            bytes: minimal_pdf::pdf_with_no_pages(),
            password: None,
            // Not `Ok { page_count: 0 }`. A document with no pages is not a document that
            // opened successfully, and the whole of ADR 0006 requirement 6 is about not
            // letting "zero pages" be a success.
            expect: Expect::Err(ErrorKind::Malformed),
        },
        Fixture {
            name: "encrypted-no-password",
            filename: "encrypted.pdf",
            bytes: minimal_pdf::pdf_encrypted_with_unusable_credentials(),
            password: None,
            expect: Expect::Err(ErrorKind::PasswordRequired),
        },
        Fixture {
            name: "encrypted-wrong-password",
            filename: "encrypted.pdf",
            bytes: minimal_pdf::pdf_encrypted_with_unusable_credentials(),
            password: Some("not the password"),
            expect: Expect::Err(ErrorKind::PasswordRequired),
        },
    ];

    let mut cases = Vec::new();
    for fixture in files {
        let path = fixtures.join(fixture.filename);
        std::fs::write(&path, &fixture.bytes)
            .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
        cases.push(Case {
            name: fixture.name.to_owned(),
            file: format!("fixtures/{}", fixture.filename),
            sha256: sha256_hex(&fixture.bytes),
            password: fixture.password.map(str::to_owned),
            expect: fixture.expect,
        });
        println!("wrote {} ({} bytes)", path.display(), fixture.bytes.len());
    }

    let expectations = Expectations {
        schema: 1,
        generated_by: GENERATED_BY.to_owned(),
        cases,
    };
    let json = serde_json::to_string_pretty(&expectations).expect("the schema should serialise");
    let out = dir.join("expectations.json");
    std::fs::write(&out, format!("{json}\n"))
        .unwrap_or_else(|e| panic!("writing {}: {e}", out.display()));
    println!("wrote {}", out.display());
}
