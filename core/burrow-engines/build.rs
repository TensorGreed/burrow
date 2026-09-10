//! Emit the link configuration for the vendored native engines.
//!
//! Only does anything when the `native-engines` feature is on **and** the target is
//! Linux. That keeps `cargo clippy --all-features`, the wasm build, and the mobile
//! `cargo check` jobs working untouched — none of them link, so none of them need
//! engines present.
//!
//! # No network access
//!
//! This script reads `engines/pins.toml` and the vendored tree. It never downloads.
//! `engines/fetch.sh` is the only thing in the build that touches the network.
//!
//! # It fails, it does not skip
//!
//! If the feature is on and the libraries are missing or their checksums do not match,
//! this script errors out. Silently degrading to "no engines, tests pass" would hide
//! exactly the coverage we most need.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

fn main() {
    // Re-run when the pins or the feature change. Deliberately not watching all of
    // `engines/vendor/`: it is large, and the checksum below is what actually protects us.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_NATIVE_ENGINES");
    // Declared unconditionally: lib.rs mentions this cfg whether or not the feature is
    // on, and an undeclared cfg is a warning, which CI treats as an error.
    println!("cargo:rustc-check-cfg=cfg(burrow_native_engines)");

    if std::env::var_os("CARGO_FEATURE_NATIVE_ENGINES").is_none() {
        return;
    }

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "linux" {
        // Only Linux natives are pinned (ADR 0004). iOS and Android acquisition is M3/M4.
        // Emitting nothing is right: `cargo check` for those targets does not link.
        println!(
            "cargo:warning=native-engines is enabled but target_os is `{target_os}`; \
             no native engines are pinned for it, so no link flags were emitted"
        );
        return;
    }

    // `fail` rather than `expect`: the workspace denies `expect_used`, and a build script
    // that aborts with a clear message beats one that panics with a backtrace.
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .unwrap_or_else(|_| fail("cargo did not set CARGO_MANIFEST_DIR")),
    );
    // core/burrow-engines -> repository root.
    let repo_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| fail("could not locate the repository root from CARGO_MANIFEST_DIR"));

    let pins = repo_root.join("engines/pins.toml");
    println!("cargo:rerun-if-changed={}", pins.display());

    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let (pin_key, arch_dir) = match arch.as_str() {
        "aarch64" => ("linux-arm64", "native-aarch64"),
        "x86_64" => ("linux-x64", "native-x86_64"),
        other => fail(&format!(
            "no native engines are pinned for target_arch `{other}`; \
             only aarch64 and x86_64 are (see engines/pins.toml)"
        )),
    };

    let lib_dir = repo_root.join("engines/vendor").join(arch_dir).join("lib");
    if !lib_dir.is_dir() {
        fail(&format!(
            "the `native-engines` feature is on but {} does not exist.\n\
             Run:  engines/fetch.sh && engines/build-native.sh",
            lib_dir.display()
        ));
    }

    // Verify the library we are about to link, against the pin for this architecture.
    // The tarball hash in pins.toml says nothing about what was unpacked, so the
    // extracted file is pinned separately as `so_sha256`.
    let expected = so_sha256_for(&pins, pin_key);
    let pdfium_so = lib_dir.join("libpdfium.so");
    verify(&pdfium_so, &expected);

    for name in ["libqpdf.a", "libz.a", "libjpeg.a"] {
        let p = lib_dir.join(name);
        if !p.is_file() {
            fail(&format!(
                "{} is missing. Run: engines/build-native.sh",
                p.display()
            ));
        }
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    // PDFium is dynamic: no static archive is published for any platform (ADR 0004).
    println!("cargo:rustc-link-lib=dylib=pdfium");

    // qpdf and its dependencies are static, and order matters for static archives:
    // qpdf depends on z and jpeg, so it must come first.
    println!("cargo:rustc-link-lib=static=qpdf");
    println!("cargo:rustc-link-lib=static=z");
    println!("cargo:rustc-link-lib=static=jpeg");
    // qpdf is C++; rustc drives the link through `cc`, which does not add the C++
    // runtime. Built with clang++, which uses libstdc++ on Linux by default.
    println!("cargo:rustc-link-lib=dylib=stdc++");

    // Bake the rpath so the loader finds *our* libpdfium.so rather than searching.
    // Computed from CARGO_MANIFEST_DIR, never hardcoded.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    // Emit DT_RPATH rather than DT_RUNPATH. DT_RUNPATH is overridable by
    // LD_LIBRARY_PATH; DT_RPATH is not, so the pinned copy cannot be shadowed by an
    // environment variable. This is dev/CI-only linkage, so the usual objection to
    // DT_RPATH (it takes precedence away from the user) is exactly what we want.
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");

    // Cargo propagates `rustc-link-lib` to every downstream binary but NOT
    // `rustc-link-arg`, so burrow-core's and burrow-ffi's test binaries end up with a
    // DT_NEEDED on libpdfium.so and no rpath to find it. (`--as-needed` does not save
    // them: the rlib's FPDF references are linked in, so the dependency is real.)
    //
    // .cargo/config.toml gives every Linux binary `-Wl,-rpath,$ORIGIN`, which resolves
    // to the directory the binary sits in. Put a verified copy there so it resolves.
    copy_beside_binaries(&pdfium_so);

    // Let the tests assert that the loaded library is the pinned one.
    println!("cargo:rustc-env=BURROW_PDFIUM_SO={}", pdfium_so.display());
    println!("cargo:rustc-env=BURROW_PDFIUM_SHA256={expected}");
    println!("cargo:rustc-cfg=burrow_native_engines");
}

/// Copy the verified library next to the test binaries so `$ORIGIN` resolves.
///
/// `OUT_DIR` is `<target>/[<triple>/]<profile>/build/<pkg>-<hash>/out`, so three parents
/// up is the profile directory in both native and cross layouts. Copies are only made
/// from the file this script has already checksum-verified.
fn copy_beside_binaries(verified: &Path) {
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let profile_dir = match PathBuf::from(&out_dir)
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
    {
        Some(dir) => dir.to_path_buf(),
        // Not fatal: burrow-engines' own binary still has the absolute rpath below, so
        // this only affects downstream crates. Warn rather than fail the build.
        None => {
            println!("cargo:warning=could not derive the profile directory from OUT_DIR");
            return;
        }
    };

    for dir in [profile_dir.join("deps"), profile_dir] {
        if !dir.is_dir() {
            continue;
        }
        let dest = dir.join("libpdfium.so");
        // Skip if a same-size copy is already there, so repeated builds do not churn.
        // Size rather than content: the test hashes whatever the loader maps, so a
        // mismatched copy fails loudly there rather than being silently tolerated.
        if let (Ok(a), Ok(b)) = (std::fs::metadata(verified), std::fs::metadata(&dest))
            && a.len() == b.len()
        {
            continue;
        }
        if let Err(e) = std::fs::copy(verified, &dest) {
            println!(
                "cargo:warning=could not copy libpdfium.so to {}: {e}",
                dir.display()
            );
        }
    }
}

/// Extract `so_sha256` for one pdfium artifact from `engines/pins.toml`.
///
/// A targeted scan rather than a TOML parser: this needs exactly one value, and pulling
/// in `toml` plus `serde` as build dependencies to read one string is a poor trade. It
/// fails closed — an unparseable or missing pin aborts the build rather than skipping
/// verification.
fn so_sha256_for(pins: &Path, artifact_key: &str) -> String {
    let text = std::fs::read_to_string(pins)
        .unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", pins.display())));

    let section = format!("[pdfium.artifacts.{artifact_key}]");
    let start = text
        .find(&section)
        .unwrap_or_else(|| fail(&format!("{} has no {section}", pins.display())));

    // Stop at the next section header so we cannot pick up a neighbour's value.
    let rest = &text[start + section.len()..];
    let end = rest.find("\n[").unwrap_or(rest.len());

    for line in rest[..end].lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("so_sha256") {
            let hash = value
                .trim_start()
                .strip_prefix('=')
                .map(|v| v.trim().trim_matches('"'))
                .unwrap_or_else(|| fail(&format!("malformed so_sha256 line in {section}")));
            if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                fail(&format!("so_sha256 in {section} is not a sha256: {hash:?}"));
            }
            return hash.to_owned();
        }
    }
    fail(&format!("{section} has no so_sha256 pin"))
}

/// Abort the build with a message. Returns `!` so it can be used in expression position.
fn fail(message: &str) -> ! {
    // `cargo:error=` renders as a build error rather than a panic backtrace.
    for line in message.lines() {
        println!("cargo:error={line}");
    }
    std::process::exit(1);
}

/// Fail the build unless `path` hashes to `expected`.
fn verify(path: &Path, expected: &str) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        fail(&format!(
            "cannot read {}: {e}\nRun: engines/fetch.sh && engines/build-native.sh",
            path.display()
        ))
    });
    let got = hex(&Sha256::digest(&bytes));
    if got != expected {
        fail(&format!(
            "CHECKSUM MISMATCH for {}\n  expected {expected}\n  got      {got}\n\
             The vendored tree does not match engines/pins.toml. Re-run engines/fetch.sh \
             and engines/build-native.sh; if it still differs, do not build.",
            path.display()
        ));
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}
