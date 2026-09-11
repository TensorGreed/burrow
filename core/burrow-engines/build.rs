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
    let expected = pin_sha256(&pins, &format!("pdfium.artifacts.{pin_key}"), "so_sha256");
    let pdfium_so = lib_dir.join("libpdfium.so");
    verify(&pdfium_so, &expected);
    // Watch the four libraries by name. Not the whole vendor tree -- that is hundreds of
    // megabytes -- but enough that a rebuild re-runs the verification below instead of
    // trusting a stale fingerprint.
    println!("cargo:rerun-if-changed={}", pdfium_so.display());

    // `fuzz/libqpdf.a` is the ASan+fuzzer-instrumented qpdf. It is not linked by this
    // build -- the fuzz targets select it with a `-L native=` override -- but it IS linked
    // into fuzz binaries, and until M1 PR 3 nothing checksummed it at all. A CI cache
    // restoring a tree nobody verified is precisely what the manifest exists to catch, and
    // an unverified archive is no less dangerous for being used only by a fuzzer.
    let archives = ["libqpdf.a", "libz.a", "libjpeg.a", "fuzz/libqpdf.a"];
    for name in archives {
        let p = lib_dir.join(name);
        println!("cargo:rerun-if-changed={}", p.display());
        if !p.is_file() {
            fail(&format!(
                "{} is missing. Run: engines/build-native.sh",
                p.display()
            ));
        }
    }

    // The static archives are built locally, so they have no upstream hash to pin. What
    // build-native.sh CAN do is record what it produced; verify against that, so a tree
    // modified after the build (or restored from a poisoned CI cache) is caught.
    verify_build_manifest(&lib_dir, &archives);

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
    //
    // Validated first: the compiler driver splits `-Wl,` arguments on commas, so a
    // checkout path containing a comma would inject arbitrary linker options; and
    // `Path::display()` is lossy, so a non-UTF-8 path would silently produce a rpath
    // pointing somewhere else entirely.
    let lib_dir_str = match lib_dir.to_str() {
        Some(s) if !s.contains(',') => s,
        Some(s) => fail(&format!(
            "the engine directory path contains a comma, which would inject linker \
             options via -Wl,: {s}\nMove the checkout somewhere without a comma in its path."
        )),
        None => fail("the engine directory path is not valid UTF-8; refusing to build"),
    };
    println!("cargo:rustc-link-search=native={lib_dir_str}");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_dir_str}");
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
    copy_beside_binaries(&pdfium_so, &expected);

    // Let the tests assert that the loaded library is the pinned one.
    println!("cargo:rustc-env=BURROW_PDFIUM_SO={}", pdfium_so.display());
    println!("cargo:rustc-env=BURROW_PDFIUM_SHA256={expected}");
    // So the link test asserts against the PIN rather than a literal it can drift from.
    println!(
        "cargo:rustc-env=BURROW_QPDF_VERSION={}",
        pin_value(&pins, "qpdf", "version")
    );
    println!("cargo:rustc-cfg=burrow_native_engines");
}

/// Copy the verified library next to the test binaries so `$ORIGIN` resolves.
///
/// `OUT_DIR` is `<target>/[<triple>/]<profile>/build/<pkg>-<hash>/out`, so three parents
/// up is the profile directory in both native and cross layouts. Copies are only made
/// from the file this script has already checksum-verified.
fn copy_beside_binaries(verified: &Path, expected: &str) {
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

        // Re-copy unless the existing file's DIGEST already matches. Comparing lengths
        // would be a real hole: `$ORIGIN` sorts ahead of the vendor path in DT_RPATH, so
        // this copy is what actually loads. A hostile library padded to the same size
        // would then be executed by every binary in the workspace, and only
        // burrow-engines' own provenance test would notice.
        if file_sha256(&dest).as_deref() == Some(expected) {
            continue;
        }
        if let Err(e) = std::fs::copy(verified, &dest) {
            println!(
                "cargo:warning=could not copy libpdfium.so to {}: {e}",
                dir.display()
            );
            continue;
        }
        // Prove the copy landed intact rather than assuming fs::copy succeeded silently.
        if file_sha256(&dest).as_deref() != Some(expected) {
            fail(&format!(
                "copied libpdfium.so to {} but its digest does not match the pin",
                dest.display()
            ));
        }
    }
}

/// Verify the locally built archives against the manifest `build-native.sh` wrote.
///
/// These have no upstream hash to pin: we build them, and the builds are not
/// bit-reproducible. What this catches is a tree changed *after* the build -- including
/// one restored from a CI cache that no longer matches what produced it.
///
/// It does NOT establish upstream provenance for the archives, and cannot: an attacker
/// who can rewrite the cached tree can rewrite the manifest inside it too. The upstream
/// guarantee for these comes from `engines/fetch.sh` verifying their SOURCE tarballs.
fn verify_build_manifest(lib_dir: &Path, archives: &[&str]) {
    let manifest = lib_dir.join("BUILD_MANIFEST.sha256");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let text = match std::fs::read_to_string(&manifest) {
        Ok(t) => t,
        Err(e) => fail(&format!(
            "{} is missing or unreadable ({e}).\nRun: engines/build-native.sh",
            manifest.display()
        )),
    };

    for name in archives {
        let want = text
            .lines()
            .filter_map(|l| {
                let (hash, file) = l.split_once("  ")?;
                (file.trim() == *name).then_some(hash.trim())
            })
            .next()
            .unwrap_or_else(|| {
                fail(&format!(
                    "{} does not record {name}. Re-run engines/build-native.sh",
                    manifest.display()
                ))
            });
        verify(&lib_dir.join(name), want);
    }
}

/// sha256 of a file, or `None` if it cannot be read.
fn file_sha256(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| hex(&Sha256::digest(&b)))
}

/// Extract one scalar value from a `[section]` of `engines/pins.toml`.
///
/// A targeted scan rather than a TOML parser: this needs exactly one value, and pulling
/// in `toml` plus `serde` as build dependencies to read one string is a poor trade. It
/// fails closed — an unparseable or missing pin aborts the build rather than skipping
/// verification.
fn pin_value(pins: &Path, section_name: &str, key: &str) -> String {
    let text = std::fs::read_to_string(pins)
        .unwrap_or_else(|e| fail(&format!("cannot read {}: {e}", pins.display())));

    let section = format!("[{section_name}]");

    // Match the header only at the START of a line, and skip comments. A bare substring
    // search would also match inside a comment or a string, so a future comment
    // mentioning a section name would silently redirect this scan to the wrong table.
    let mut in_section = false;
    let mut lines_in_section = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_section = line == section;
            continue;
        }
        if in_section {
            lines_in_section.push(line);
        }
    }
    if lines_in_section.is_empty() {
        fail(&format!("{} has no {section}", pins.display()));
    }

    for line in lines_in_section {
        if let Some(value) = line.strip_prefix(key) {
            let value = value
                .trim_start()
                .strip_prefix('=')
                .map(|v| v.trim().trim_matches('"'))
                .unwrap_or_else(|| fail(&format!("malformed {key} line in {section}")));
            if value.is_empty() {
                fail(&format!("{key} in {section} is empty"));
            }
            return value.to_owned();
        }
    }
    fail(&format!("{section} has no {key} pin"))
}

/// Extract a sha256 pin, rejecting anything that is not 64 hex digits.
///
/// The shape check is the backstop that makes the targeted scan safe: if the section
/// matching ever went wrong, a value from the neighbouring table would still have to
/// look like a digest, and then fail verification against the real file.
fn pin_sha256(pins: &Path, section_name: &str, key: &str) -> String {
    let v = pin_value(pins, section_name, key);
    if v.len() != 64 || !v.chars().all(|c| c.is_ascii_hexdigit()) {
        fail(&format!("{key} in [{section_name}] is not a sha256: {v:?}"));
    }
    v
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
