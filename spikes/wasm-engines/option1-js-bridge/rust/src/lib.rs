//! Option 1: Rust on `wasm32-unknown-unknown`, driving Emscripten engine modules
//! through JS.
//!
//! Rust owns the decision logic and the error taxonomy; JS owns only the mechanical
//! call into each engine module. That split is the thing being evaluated: ADR 0002 says
//! bindings contain no logic, and this option is the one that puts pressure on it.
//!
//! Spike code. Panics and unwraps are acceptable here in a way they are not in `core/`.

use wasm_bindgen::prelude::*;

/// Mirrors the shape of `burrow_types::Error`, so the spike exercises the real mapping.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpikeError {
    Ok = 0,
    Malformed = 1,
    PasswordRequired = 2,
    Unsupported = 3,
    Internal = 4,
}

/// What an engine call returns to Rust: a page count, or a typed error.
#[wasm_bindgen]
pub struct Outcome {
    pages: i32,
    error: SpikeError,
}

#[wasm_bindgen]
impl Outcome {
    #[wasm_bindgen(getter)]
    pub fn pages(&self) -> i32 {
        self.pages
    }
    #[wasm_bindgen(getter)]
    pub fn error(&self) -> SpikeError {
        self.error
    }
    #[wasm_bindgen(getter)]
    pub fn is_ok(&self) -> bool {
        self.error == SpikeError::Ok
    }
}

// The JS bridge. Each function is a thin mechanical wrapper over one engine module:
// allocate in the engine heap, copy, call, free. No decisions live there.
//
// Declared as globals rather than an ES module snippet on purpose: the engine modules
// are loaded with `importScripts`, which exists only in a *classic* worker, so the
// whole chain has to be classic. A module worker would be tidier and cannot be used.
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "pdfiumPageCount", catch)]
    fn pdfium_page_count(bytes: &[u8]) -> Result<i32, JsValue>;

    #[wasm_bindgen(js_name = "qpdfPageCount", catch)]
    fn qpdf_page_count(bytes: &[u8]) -> Result<i32, JsValue>;
}

/// PDFium reports failure as a negative sentinel from the bridge; map it to a type.
#[wasm_bindgen]
pub fn open_with_pdfium(bytes: &[u8]) -> Outcome {
    match pdfium_page_count(bytes) {
        Ok(n) if n >= 0 => Outcome { pages: n, error: SpikeError::Ok },
        // The bridge maps FPDF_GetLastError; 4 is FPDF_ERR_PASSWORD.
        Ok(-4) => Outcome { pages: -1, error: SpikeError::PasswordRequired },
        Ok(_) => Outcome { pages: -1, error: SpikeError::Malformed },
        Err(_) => Outcome { pages: -1, error: SpikeError::Internal },
    }
}

/// qpdf returns the shim's status codes directly.
#[wasm_bindgen]
pub fn open_with_qpdf(bytes: &[u8]) -> Outcome {
    match qpdf_page_count(bytes) {
        Ok(n) if n >= 0 => Outcome { pages: n, error: SpikeError::Ok },
        Ok(-1) => Outcome { pages: -1, error: SpikeError::Malformed },
        Ok(-2) => Outcome { pages: -1, error: SpikeError::PasswordRequired },
        Ok(-3) => Outcome { pages: -1, error: SpikeError::Unsupported },
        Ok(_) => Outcome { pages: -1, error: SpikeError::Internal },
        Err(_) => Outcome { pages: -1, error: SpikeError::Internal },
    }
}

/// Deliberately panic, to prove what a Rust panic does to the worker.
///
/// On `wasm32-unknown-unknown` a panic **aborts**; there is no unwinding, so
/// `catch_unwind` cannot help. The page must detect the dead worker and respawn, which
/// is what `apps/web/CLAUDE.md` requires and what the Playwright test asserts.
#[wasm_bindgen]
pub fn deliberate_panic() {
    panic!("spike: deliberate panic to test worker recovery");
}

#[wasm_bindgen]
pub fn version() -> String {
    "spike-option1".to_owned()
}
