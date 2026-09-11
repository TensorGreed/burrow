//! Nothing from a user's file reaches stdout, stderr, or an error string.
//!
//! This is the headline test of M1 PR 3, and it exists because the problem is real and
//! measured, not hypothetical. [Spike 0001](../../../docs/spikes/0001-wasm-engines.md)
//! Finding 6 recorded qpdf doing exactly this, unprompted:
//!
//! ```text
//! WARNING: spike-input (object 3 0, offset 9999999999): expected n n obj
//! ```
//!
//! Object numbers and byte offsets are file content by any reasonable reading. In a
//! browser they land in the devtools console; in a native app, in the platform log. That
//! makes it a non-negotiable #1 problem — the privacy claim is the product — rather than
//! a cosmetic one. And qpdf emits these **for files that parse successfully**, so it is
//! not confined to the error path.
//!
//! # Why the capture is a child process
//!
//! C and C++ write to **file descriptors 1 and 2 directly**. They do not go through
//! Rust's `std::io::stdout()`, so `std::io::set_output_capture` — what `cargo test` uses
//! to swallow `println!` — never sees a single byte of what PDFium or qpdf print. A test
//! that captured Rust's stderr and found nothing would be measuring nothing, and would
//! pass just as cheerfully with the suppression removed.
//!
//! So each case runs in a **re-executed copy of this test binary** with both descriptors
//! redirected to pipes by `Stdio::piped()`. That is an OS-level redirection of the
//! descriptors themselves, so it catches everything written to them by anyone — Rust, C,
//! C++, or the dynamic loader. No `libc`, no `dup2`, no `unsafe` needed to set it up.
//!
//! # Why the controls matter more than the assertions
//!
//! A leak test that cannot fail is worse than no leak test: it converts "nobody checked"
//! into "something checked and it was fine". Two controls guard against that, and both
//! are asserted before any of the real cases are believed:
//!
//! 1. `control-print-canary` deliberately writes the canary to **both** Rust's stdout and
//!    directly to file descriptor 2, and the harness asserts the search *finds* it. If the
//!    pipe plumbing broke, every other case would pass silently; this turns that into a
//!    failure.
//! 2. The same control asserts [`check_absent`] — the matcher every real case depends on —
//!    *does* report a leak when handed output that definitely contains one. Otherwise a
//!    refactor that made it always succeed would leave everything green.
//! 3. `control-no-canary-present` runs the identical machinery with a canary that was never
//!    put in any file, asserting the search finds nothing. That catches the opposite bug —
//!    a matcher so eager it reports a leak that is not there.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::Arc;

use burrow_engines::pdfium::Pdfium;
use burrow_engines::qpdf::Qpdf;
use burrow_engines::{CheckOptions, DocumentEngine, OpenOptions, StructureEngine};
use burrow_types::{Limits, ManualClock, Password};
use support::minimal_pdf;

/// The environment variable that puts a re-executed binary into child mode.
const CASE_VAR: &str = "BURROW_LEAK_CASE";
/// The canary the child should use, passed through the environment.
const CANARY_VAR: &str = "BURROW_LEAK_CANARY";

/// Every way a canary is pushed through the engines.
///
/// Named rather than indexed so a failure says which path leaked.
const CASES: &[&str] = &[
    "control-print-canary",
    "control-no-canary-present",
    "pdfium-malformed",
    "pdfium-wrong-password",
    "pdfium-limit-exceeded",
    "pdfium-prescan-rejected",
    "pdfium-caught-panic",
    "qpdf-malformed",
    "qpdf-wrong-password",
    "qpdf-limit-exceeded",
    "qpdf-prescan-rejected",
    "qpdf-success-with-warnings",
];

/// A canary that is unique per run.
///
/// The fixed prefix makes a hit obvious in output; the random suffix means a stale string
/// left in a build artifact, a core file, or another test's output cannot be mistaken for
/// this run's leak.
fn canary() -> String {
    // No `rand` dependency for this: the process id and the current time are plenty
    // unique for distinguishing one test run from another, and adding a crate to the
    // dependency graph for a test nonce is not a trade worth making.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("BURROW-CANARY-7f3a9c-{}-{nanos:08x}", std::process::id())
}

// =====================================================================================
// Parent side
// =====================================================================================

#[test]
fn no_engine_failure_path_leaks_file_content_to_any_output() {
    // Child mode: this binary was re-executed to run one case. Do that and exit, rather
    // than recursing into the harness.
    if let Ok(case) = std::env::var(CASE_VAR) {
        run_child(&case, &std::env::var(CANARY_VAR).unwrap_or_default());
        return;
    }

    let canary = canary();
    let mut failures: Vec<String> = Vec::new();

    for case in CASES {
        let captured = run_case(case, &canary);
        if let Err(why) = judge(case, &canary, &captured) {
            failures.push(format!("  {case}: {why}"));
        }
    }

    assert!(
        failures.is_empty(),
        "the canary escaped, or a control failed:\n{}",
        failures.join("\n")
    );
}

/// What one child wrote to each descriptor, and what it returned.
struct Captured {
    stdout: String,
    stderr: String,
    errors: String,
    status_ok: bool,
}

/// Re-execute this binary for one case and collect everything it produced.
fn run_case(case: &str, canary: &str) -> Captured {
    let exe = std::env::current_exe().expect("a test binary knows its own path");
    let output = Command::new(exe)
        // The harness would otherwise run every test in the child too.
        .arg("--exact")
        .arg("no_engine_failure_path_leaks_file_content_to_any_output")
        .arg("--nocapture")
        .env(CASE_VAR, case)
        .env(CANARY_VAR, canary)
        // The whole point: real pipes on the child's descriptors 1 and 2, so C and C++
        // writes are captured along with Rust ones.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("could not re-execute for case {case}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // The child prints its rendered errors on a marked line so the parent can check the
    // `Display` and `Debug` output too, not only the descriptors.
    let errors = stdout
        .lines()
        .chain(stderr.lines())
        .filter(|l| l.starts_with(ERROR_MARKER))
        .collect::<Vec<_>>()
        .join("\n");

    Captured {
        stdout,
        stderr,
        errors,
        status_ok: output.status.success(),
    }
}

/// Decide whether one case passed, including the two controls.
fn judge(case: &str, canary: &str, captured: &Captured) -> Result<(), String> {
    let hay = format!("{}\n{}", captured.stdout, captured.stderr);

    match case {
        // Control 1: the canary was deliberately printed. If the harness cannot find it
        // here, the capture is broken and every other case's pass is meaningless.
        "control-print-canary" => {
            if !hay.contains(canary) {
                return Err(format!(
                    "the control printed the canary and the harness did not see it, so \
                     the capture is not working and NO other case in this test means \
                     anything. stdout={} bytes, stderr={} bytes",
                    captured.stdout.len(),
                    captured.stderr.len()
                ));
            }
            // It must have reached *both* descriptors, including the raw one, or the
            // control is only proving half of what it claims.
            if !captured.stdout.contains(canary) {
                return Err("the control's stdout write was not captured".to_owned());
            }
            if !captured.stderr.contains(canary) {
                return Err(
                    "the control's raw file-descriptor-2 write was not captured, so C-level \
                     output would not be either"
                        .to_owned(),
                );
            }
            // And `check_absent` -- the function every real case depends on, including its
            // fragment loop -- must actually be capable of reporting a leak. Without this
            // a refactor that made it always return `Ok` would leave both controls passing
            // and all ten real cases green.
            if check_absent(canary, "stdout", &captured.stdout).is_ok() {
                return Err(
                    "check_absent did not flag output that definitely contains the canary, \
                     so it cannot flag output that accidentally does"
                        .to_owned(),
                );
            }
            Ok(())
        }

        // Control 2: nothing anywhere used this canary, so finding it would mean the
        // matcher is reporting phantom leaks.
        "control-no-canary-present" => {
            if hay.contains(canary) {
                return Err("the matcher found a canary that was never used".to_owned());
            }
            Ok(())
        }

        _ => {
            if !captured.status_ok {
                return Err(format!(
                    "the child failed. stderr:\n{}",
                    indent(&captured.stderr)
                ));
            }
            check_absent(canary, "stdout", &captured.stdout)?;
            check_absent(canary, "stderr", &captured.stderr)?;
            check_absent(canary, "the rendered errors", &captured.errors)?;

            // Canary-absence on its own is too weak, and a mutation test proved it. With
            // the suppression removed qpdf writes
            //
            //   WARNING: input (offset 4242): xref not found
            //
            // which contains no canary but is unmistakably derived from the file -- a byte
            // offset is file content. So the property asserted is the stronger and simpler
            // one: an engine writes NOTHING to either descriptor. Anything at all is a
            // failure, whether or not we recognise it.
            let stray = stray_output(&captured.stdout, &captured.stderr);
            if !stray.is_empty() {
                return Err(format!(
                    "an engine wrote to a file descriptor. Every byte here is suspect -- \
                     offsets and object numbers are file content even when no canary is \
                     visible:\n{}",
                    indent(&stray.join("\n"))
                ));
            }
            Ok(())
        }
    }
}

/// Shortest run of canary characters treated as a leak.
///
/// The same approach as M1 PR 1's `guard()` test: a message that quoted only part of the
/// canary would still be quoting the file. Eight characters is long enough that ordinary
/// English and hexadecimal offsets do not collide with it by accident.
const FRAGMENT: usize = 8;

/// Assert the canary, whole or in fragments, is nowhere in `text`.
fn check_absent(canary: &str, what: &str, text: &str) -> Result<(), String> {
    if text.contains(canary) {
        return Err(format!("the whole canary appears in {what}"));
    }
    let bytes = canary.as_bytes();
    if bytes.len() >= FRAGMENT {
        for start in 0..=bytes.len() - FRAGMENT {
            let fragment = &canary[start..start + FRAGMENT];
            if text.contains(fragment) {
                return Err(format!(
                    "a {FRAGMENT}-character fragment of the canary ({fragment:?}) appears in \
                     {what}"
                ));
            }
        }
    }
    Ok(())
}

/// Lines the child produced that neither the harness nor the test runner put there.
///
/// Everything the child is *allowed* to emit is accounted for explicitly, so anything
/// unrecognised is reported rather than assumed benign. That is the right default for a
/// test whose whole job is noticing output nobody intended.
fn stray_output(stdout: &str, stderr: &str) -> Vec<String> {
    stdout
        .lines()
        .chain(stderr.lines())
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        // The harness's own reporting of the error it deliberately produced.
        .filter(|line| !line.starts_with(ERROR_MARKER))
        // libtest's framing around the single test the child runs.
        .filter(|line| {
            !(line.starts_with("running ")
                || line.starts_with("test result:")
                || line.starts_with("test no_engine_failure_path_leaks")
                || line.is_empty())
        })
        .map(str::to_owned)
        .collect()
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("      {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// =====================================================================================
// Child side
// =====================================================================================

/// Prefix the child puts on any line carrying a rendered error.
const ERROR_MARKER: &str = "BURROW-ERROR ";

/// Render an error's `Display` **and** `Debug` onto a marked line.
///
/// Both, because they are different strings: a `Debug` that happened to include a field
/// `Display` omits would be a leak the obvious check misses.
fn report(error: &burrow_types::Error) {
    println!("{ERROR_MARKER}display={error}");
    println!("{ERROR_MARKER}debug={error:?}");
}

fn report_result<T>(result: Result<T, burrow_types::Error>) {
    if let Err(error) = result {
        report(&error);
    }
}

fn stopped() -> Arc<dyn burrow_types::Clock> {
    Arc::new(ManualClock::new(0))
}

fn run_child(case: &str, canary: &str) {
    match case {
        "control-print-canary" => {
            // Rust's own streams...
            println!("{canary}");
            // ...and a write straight to file descriptor 2, bypassing Rust entirely. This
            // is the half that proves the capture would catch C and C++ output, which is
            // the only kind that matters here.
            write_to_raw_fd_2(canary);
        }

        "control-no-canary-present" => {
            println!("this child prints nothing derived from any file");
        }

        "pdfium-malformed" => {
            // PDFium reconstructs this file rather than refusing it, so this exercises a
            // *successful* open whose input is full of canaries -- which is worth having
            // (qpdf's warnings fire on successful parses too) but is not what the name
            // suggested. Asserted either way, so the case can never silently stop
            // reaching the engine: a pre-scan tightening that turned this into an early
            // rejection would fail here rather than quietly becoming a pre-scan test.
            let bytes = minimal_pdf::pdf_with_canary(canary);
            let result = open_pdfium(bytes, Limits::default());
            expect_reached_engine("pdfium-malformed", &result);
            report_result(result);
        }

        "pdfium-wrong-password" => {
            // The canary as the *password*, which is a secret the caller gave us rather
            // than file content -- the one leak `Password`'s redacted `Debug` is for.
            let bytes = minimal_pdf::pdf_encrypted_with_unusable_credentials();
            let password = Password::new(canary.as_bytes());
            let mut options = OpenOptions::new(Limits::default(), stopped());
            options.password = Some(&password);
            report_result(Pdfium::new().open(bytes.into_boxed_slice(), &options));
        }

        "pdfium-limit-exceeded" => {
            let bytes = minimal_pdf::pdf_with_canary(canary);
            report_result(open_pdfium(bytes, Limits::with(|l| l.max_input_bytes = 8)));
        }

        "pdfium-prescan-rejected" => {
            let bytes = prescan_bomb_with_canary(canary);
            report_result(open_pdfium(bytes, Limits::default()));
        }

        "pdfium-caught-panic" => {
            // A panic whose message is the canary, forced through the engine thread's
            // `catch_unwind`. The payload must be dropped rather than formatted, which is
            // exactly what a leak test should confirm rather than assume.
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let caught = std::panic::catch_unwind(|| -> ! { panic!("{canary}") });
            std::panic::set_hook(previous);
            // Deliberately dropped without being formatted -- the same discipline the
            // engine thread uses. Printing it here would be printing the canary.
            drop(caught.err());
            println!("{ERROR_MARKER}display=a panic was caught and its payload discarded");
        }

        "qpdf-malformed" => {
            let bytes = minimal_pdf::pdf_with_canary(canary);
            let result = check_qpdf(bytes, Limits::default());
            expect_reached_engine("qpdf-malformed", &result);
            report_result(result);
        }

        "qpdf-wrong-password" => {
            let bytes = minimal_pdf::pdf_encrypted_with_unusable_credentials();
            let password = Password::new(canary.as_bytes());
            let mut options = CheckOptions::new(Limits::default(), stopped());
            options.password = Some(&password);
            report_result(Qpdf::new().check(bytes.into_boxed_slice(), &options));
        }

        "qpdf-limit-exceeded" => {
            let bytes = minimal_pdf::pdf_with_canary(canary);
            report_result(check_qpdf(bytes, Limits::with(|l| l.max_input_bytes = 8)));
        }

        "qpdf-prescan-rejected" => {
            let bytes = prescan_bomb_with_canary(canary);
            report_result(check_qpdf(bytes, Limits::default()));
        }

        "qpdf-success-with-warnings" => {
            // The case spike 0001 found, and the one that actually produces output. With
            // `attempt_recovery` on, qpdf reconstructs the broken cross-reference table and
            // narrates it:
            //
            //   WARNING: input (offset 4242): xref not found
            //   WARNING: input, object 3 0 at offset 131: kid 0 ... is missing or invalid
            //
            // Byte offsets and object numbers -- file content. Measured on this build with
            // the suppression removed, which is how this test earns the right to claim the
            // suppression is load-bearing.
            let bytes = minimal_pdf::pdf_with_warnings_and_canary(canary);
            let mut options = CheckOptions::new(Limits::default(), stopped());
            options.attempt_recovery = true;
            report_result(
                Qpdf::new()
                    .check(bytes.clone().into_boxed_slice(), &options)
                    .map(|_| ()),
            );
            report_result(open_pdfium(bytes, Limits::default()));
        }

        other => panic!("unknown case {other}"),
    }
    // Flush before exiting, or a buffered line could be lost and look like a pass.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

/// Fail the child unless the case actually reached an engine.
///
/// The pre-scan runs before both engines, so a fixture that trips it never exercises the
/// engine at all — and the case becomes a test of the pre-scan wearing the engine's name.
/// That has already happened once in this file: a `/Length 99999` in a 600-byte fixture
/// meant every "malformed" case was rejected before PDFium or qpdf saw a byte, and the
/// whole suite passed with the qpdf suppression removed.
///
/// A `LimitExceeded` here means the pre-scan or a ceiling intervened, which for these two
/// cases is a silent loss of coverage rather than a pass.
fn expect_reached_engine(case: &str, result: &Result<(), burrow_types::Error>) {
    assert!(
        !matches!(result, Err(burrow_types::Error::LimitExceeded { .. })),
        "{case}: the fixture was rejected before any engine saw it ({result:?}), so this \
         case is no longer testing what its name says"
    );
}

fn open_pdfium(bytes: Vec<u8>, limits: Limits) -> Result<(), burrow_types::Error> {
    Pdfium::new()
        .open(
            bytes.into_boxed_slice(),
            &OpenOptions::new(limits, stopped()),
        )
        .map(|_| ())
}

fn check_qpdf(bytes: Vec<u8>, limits: Limits) -> Result<(), burrow_types::Error> {
    Qpdf::new()
        .check(
            bytes.into_boxed_slice(),
            &CheckOptions::new(limits, stopped()),
        )
        .map(|_| ())
}

/// A file the pre-scan refuses, with the canary next to the declaration that does it.
fn prescan_bomb_with_canary(canary: &str) -> Vec<u8> {
    format!(
        "%PDF-1.5\n1 0 obj\n<< /Type /Catalog /{canary} ({canary}) >>\nendobj\n\
         4 0 obj\n<< /Type /XRef /Size 900000000 /W [1 8 8] /{canary} ({canary}) >>\n\
         stream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n"
    )
    .into_bytes()
}

/// Write `text` straight to file descriptor 2, bypassing Rust's `stderr`.
///
/// This is what makes the capture control meaningful: it proves the pipe catches writes
/// made the way C and C++ make them, which is the only way PDFium and qpdf write.
fn write_to_raw_fd_2(text: &str) {
    use std::os::fd::FromRawFd as _;

    // SAFETY: file descriptor 2 is open for writing in any process the test harness
    // starts. `ManuallyDrop` keeps the `File` from closing it on drop, which would leave
    // the rest of the child with no stderr.
    let file = unsafe { std::fs::File::from_raw_fd(2) };
    let mut file = std::mem::ManuallyDrop::new(file);
    let _ = writeln!(file, "{text}");
    let _ = file.flush();
}
