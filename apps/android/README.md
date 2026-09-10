# burrow for Android

Not started. Lands in **M3**: a native Jetpack Compose app over the shared Rust core via
uniffi. No WebView wrapper — see [ADR 0002](../../docs/adr/0002-rust-core-and-bindings.md).

Platform services used from the app layer, not the core: ML Kit (on-device text
recognition) and Media3 (video).

Prerequisites when M3 starts, none of which are set up on the current dev machine:
Android SDK, NDK, and a JDK 17 or newer (this machine has JDK 8).
