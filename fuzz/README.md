# fuzz

`cargo-fuzz` targets. One per parser entry point — that is a requirement, not a goal:
every input is treated as hostile.

Its own workspace (excluded from the root `Cargo.toml`) because cargo-fuzz requires that
and needs a nightly toolchain.

```bash
rustup toolchain install nightly
cargo install --locked cargo-fuzz

cargo +nightly fuzz list
cargo +nightly fuzz run <target> -- -max_total_time=60
```

A target must never panic, abort, hang, or allocate past its `Limits` — regardless of
input. A crash is a bug even if the input is nonsense: nonsense input is the case that
matters.

Findings go in `fuzz/artifacts/` (gitignored). When a crash is found, minimise it, add
the minimised input to the corpus, fix the bug, and add a unit test asserting the typed
error it should now produce.

No real targets yet; they land in M1 with the first operations.
