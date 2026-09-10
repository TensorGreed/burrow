# tools

Headless scripts for corpus work, regression runs, and release plumbing. Everything here
must run without a display and without network access to user file content, so it can be
driven on the self-hosted `linux-arm64` regression machine over SSH.

| Script | Purpose | State |
|---|---|---|
| `corpus-fetch.sh` | Download and verify the corpora in `corpus/manifest.toml` | stub |
| `corpus-run.sh` | Run a corpus set through the core and report failures | stub |
| `visual-diff.sh` | Render before/after and compare pages pixel-wise | stub |

The stubs exit non-zero with an explanation rather than pretending to work. They are
implemented in M1, when there is an operation to run them against.
