---
name: m1-ci-check-vacuity
description: Measured facts about symbol-table checks and emsdk availability in burrow CI — how a "green" artifact check can examine nothing or the wrong subset
metadata:
  type: project
---

Measured 2026-09-12 while reviewing `tools/check-qpdf-crypto.sh` (branch
`ci/harden-python-checks-and-crypto-assert`). These are empirical, not derivable by reading:

- `nm --defined-only libqpdf.a` hides **5070** undefined refs, including `deflate`. Any
  check for *external* library symbols (gnutls_, EVP_*, SSL_CTX_new) over a static archive
  must run **without** `--defined-only`, or it is structurally vacuous — those symbols are
  defined in the external library, never in the archive. Removing the flag produced zero
  false positives on all three shipped libqpdf archives.
- GNU `nm` on an archive with some unreadable members **prints the readable ones and exits
  0**, with `file format not recognized` on stderr only. So "non-empty output" is not
  evidence the whole archive was read. Cross-check against `ar t | wc -l`.
- `nm` also prints member-name header lines (`QPDFCrypto_native.cc.o:`), so an unanchored
  grep for a provider can be satisfied by a filename with no symbol behind it.
- Plain `nm` on a wasm archive yields **0 lines, exit 0** (not an error) — `llvm-nm` is
  required. In ci.yml's `web` job **`EMSDK` is never exported and emsdk_env.sh is sourced
  only inside `build-wasm.sh`'s own shell**; the "Cache emsdk" step is itself gated on the
  wasm-engines cache miss, so on a warm-cache PR emsdk is not installed at all.
- `engines/vendor/wasm/lib/qpdf.wasm` (the linked, shipped module) is a separate artifact
  from `libqpdf.a` and is scannable with `strings -a | grep -i gnutls`.

**Why:** the repo's stated rule is that a check which silently examines nothing is worse
than no check; these are the concrete mechanisms by which that happens here.

**How to apply:** when reviewing any artifact-inspection check, ask what its reader does on
a partial or unreadable input, and whether the artifact inspected is the artifact shipped.

See [[m1-engine-supply-chain]], [[user-role]].
