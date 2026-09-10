# Architecture decision records

One file per decision, numbered, never rewritten. See
[0001](0001-record-architecture-decisions.md) for the process and
[`template.md`](template.md) for the shape.

| # | Decision | Status |
|---|---|---|
| [0001](0001-record-architecture-decisions.md) | Record architecture decisions in ADRs | Accepted |
| [0002](0002-rust-core-and-bindings.md) | A shared Rust core with uniffi and wasm bindings | Accepted, amended by [0009](0009-web-panic-contract-and-binding-boundary.md) |
| [0003](0003-permissive-licensing.md) | Permissive licenses only, enforced by CI | Superseded by [0008](0008-widened-licence-allowlist.md) |
| [0004](0004-native-engines.md) | Native engine selection | Accepted |
| [0005](0005-web-stack.md) | Astro with Svelte islands for the web app | Accepted |
| [0006](0006-wasm-linking-strategy.md) | WASM linking strategy for the C/C++ engines | Accepted (option 1) |
| [0007](0007-limit-enforcement-per-platform.md) | Per-platform enforcement of time and memory limits | Proposed |
| [0008](0008-widened-licence-allowlist.md) | Widened licence allowlist for bundled native engines | Accepted |
| [0009](0009-web-panic-contract-and-binding-boundary.md) | The web panic contract, and where the binding boundary really is | Accepted |
