# burrow for iOS

Not started. Lands in **M4**: a native SwiftUI app over the shared Rust core via uniffi.
No WebView wrapper — see [ADR 0002](../../docs/adr/0002-rust-core-and-bindings.md).

Platform services used from the app layer, not the core: PDFKit, Vision (on-device text
recognition), and AVFoundation (video).

Requires Xcode on macOS, so this milestone cannot be built on the Linux dev machine.
