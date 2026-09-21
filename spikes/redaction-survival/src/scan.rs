//! Instruments I1 and I2: find a canary in a file's bytes.
//!
//! I1 is the naive check -- the one ADR 0022 describes when it says "a scan for the literal
//! string finds the literal string". I2 is what `core/burrow-ops/tests/split_no_leak.rs`
//! actually does: expand the document with `qpdf --qdf --object-streams=disable` first, so a
//! canary sitting inside a flate stream is in the bytes at all.
//!
//! The encodings are taken from `split_no_leak.rs::contains`, which learned them the hard way:
//! a form field's value is written UTF-16BE, and qpdf may write a string as hex. A scan that
//! knows only ASCII reports silence for the wrong reason.

use std::path::Path;
use std::process::Command;

/// Every spelling of `needle` a PDF might carry.
fn spellings(needle: &str) -> Vec<Vec<u8>> {
    let ascii = needle.as_bytes().to_vec();
    let utf16: Vec<u8> = needle
        .encode_utf16()
        .flat_map(|u| u.to_be_bytes())
        .collect();
    let mut bom = vec![0xFE, 0xFF];
    bom.extend_from_slice(&utf16);

    let hex = |bytes: &[u8], upper: bool| -> Vec<u8> {
        bytes
            .iter()
            .flat_map(|b| {
                let s = if upper {
                    format!("{b:02X}")
                } else {
                    format!("{b:02x}")
                };
                s.into_bytes()
            })
            .collect()
    };

    vec![
        ascii,
        utf16.clone(),
        bom.clone(),
        hex(&utf16, true),
        hex(&utf16, false),
        hex(&bom, true),
        hex(&bom, false),
    ]
}

/// I1. Which spellings of the canary are present in `bytes`.
pub fn raw(bytes: &[u8], needle: &str) -> bool {
    spellings(needle)
        .iter()
        .any(|pattern| window(bytes, pattern))
}

fn window(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Expand a document the way `split_no_leak.rs` does, so a compressed canary is visible.
///
/// **No fallback.** If the CLI is missing or refuses the file, this returns `Err`. An earlier
/// harness in this repository returned the raw bytes in that case, which makes a leak test
/// report "no leak" for a document it could not read into -- `split_no_leak.rs` records that
/// as the failure this technique exists to prevent.
pub fn expand(qpdf: &Path, bytes: &[u8], scratch: &Path) -> Result<Vec<u8>, String> {
    let input = scratch.join("expand-in.pdf");
    let output = scratch.join("expand-out.pdf");
    std::fs::write(&input, bytes).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&output);
    let result = Command::new(qpdf)
        .args(["--qdf", "--object-streams=disable"])
        .arg(&input)
        .arg(&output)
        .output()
        .map_err(|e| format!("running {}: {e}", qpdf.display()))?;
    let code = result.status.code().unwrap_or(-1);
    if code != 0 && code != 3 {
        return Err(format!(
            "qpdf --qdf exited {code}: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    std::fs::read(&output).map_err(|e| e.to_string())
}
