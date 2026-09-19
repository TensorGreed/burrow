//! Link the vendored engines. Spike-only.
//!
//! This is a cut-down `core/burrow-engines/build.rs`. It deliberately does NOT verify the
//! libraries against `engines/pins.toml`: that check exists so the production build cannot
//! link something nobody pinned, and duplicating it here would be a second implementation of
//! a rule `core/CLAUDE.md` says must have exactly one. A spike measures a tree somebody has
//! already built and checked; it is not a second gate on that tree.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest.parent().unwrap().parent().unwrap();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let dir = match arch.as_str() {
        "aarch64" => "native-aarch64",
        "x86_64" => "native-x86_64",
        other => panic!("no vendored engines for target_arch `{other}`"),
    };
    let lib = repo.join("engines/vendor").join(dir).join("lib");
    assert!(
        lib.is_dir(),
        "{} is missing. Run: engines/fetch.sh && engines/build-native.sh",
        lib.display()
    );
    let lib_str = lib.to_str().expect("engine path is not UTF-8");
    assert!(!lib_str.contains(','), "engine path contains a comma");

    println!("cargo:rustc-link-lib=dylib=pdfium");
    println!("cargo:rustc-link-lib=static=qpdf");
    println!("cargo:rustc-link-lib=static=z");
    println!("cargo:rustc-link-lib=static=jpeg");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-search=native={lib_str}");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{lib_str}");
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-env=SPIKE_REPO_ROOT={}", repo.display());
}
