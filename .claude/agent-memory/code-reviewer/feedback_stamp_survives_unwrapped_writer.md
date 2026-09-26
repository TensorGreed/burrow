---
name: stamp-survives-unwrapped-writer
description: A provenance stamp beside a build output is only honest if EVERY writer of that output removes it; check whether the producer clears its out-dir (wasm-pack does not).
metadata:
  type: feedback
---

When reviewing a "stamp the artifact with its inputs" scheme (#149, and #202 for dist), ask who
else writes the output directory. wasm-pack 0.15 `create_pkg_dir` only removes `package.json`, so
a bare `wasm-pack build` (or an older branch's unwrapped ci-local) overwrites pkg/ and leaves the
newer branch's `.build-stamp` in place; back on that branch the stamp matches and the bytes are
the other branch's -- the exact #128/#130 incident. build-wasm.sh is safe only because it
`rm -rf`s its prefix. Demonstrated by begin/commit a pkg stamp, overwriting the .wasm, `check` = 0.

**Why:** inputs-only stamps (the issue forbids hashing output as the staleness test) are not
bound to the bytes; the fix is to bind them (digest of output recorded at commit, compared at
check) -- which is a binding, not the forbidden staleness criterion.

**How to apply:** for any stamp/marker beside output, read the producer's source for out-dir
clearing, and plant "stamp from A, bytes from B". Also: a derivation that text-scans .py files for
a guard literal will match its own fixture table (ci-local GUARD_CASES made checker-self-tests
"read" pkg). See [[text-scan-gate-passes-dead-rule]], [[mutate-the-wiring-not-the-policy]].
