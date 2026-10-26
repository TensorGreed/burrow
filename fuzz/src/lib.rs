//! Helpers shared by the fuzz targets. Not part of any shipped crate.

/// Report an `Error::Internal` that a fuzz target treats as a finding, then panic.
///
/// WHY A HELPER (owner decision (62)(1), 2026-10-24). The nightly found a new `reorder` crash whose
/// only description was the message inside `Error::Internal`. The public log is redacted by
/// `tools/redact-fuzz-log.py`, which withholds a panic's message line because some targets format
/// their input into it, so the message was gone from the only place anyone could read -- and the
/// input was never kept either.
///
/// The message now reaches the UNREDACTED log twice on purpose: as a `BURROW-FUZZ-FINDING` marker
/// line written straight to stderr (so it does not depend on the panic hook's formatting), and as
/// the panic message. `fuzz-nightly.yml` encrypts that log together with the crash input to a key
/// the owner holds (`tools/fuzz-seal-finding.sh`); neither line is allowed by the redactor
/// (`tools/test-fuzz-seal-finding.sh` plants one and checks).
///
/// The message is a burrow-authored string, not the input, but it may quote input-derived values;
/// treat it as sensitive exactly as the input is.
pub fn internal_error(target: &str, message: &str) -> ! {
    eprintln!("BURROW-FUZZ-FINDING target={target} internal-error: {message}");
    panic!("{target} reported an internal error: {message}");
}
