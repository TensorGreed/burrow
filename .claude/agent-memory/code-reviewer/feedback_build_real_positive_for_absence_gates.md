---
name: build-real-positive-for-absence-gates
description: Review technique -- for a "X is absent from the shipped wasm" gate, build a real module with a planted export calling X and run the real checker, rather than trusting needle choice
metadata:
  type: feedback
---

An absence gate over a wasm module is only as good as its needles, and needles chosen from one
kind of literal (refusal prefixes) miss components that never format one.

**Why:** measured reviewing #137's `check-redaction-not-in-base.sh` (43cd864). A 7-line
`#[wasm_bindgen]` export appended to `bindings/burrow-wasm/src/lib.rs`, built with
`cargo build -p burrow-wasm --target wasm32-unknown-unknown --release` (~5 s incremental), put
the rewriter's splice (+7.7 KB), the /ToUnicode narrowing (+25 KB) and the string encoder
(+5.5 KB) into the base module, and the gate passed all three. It only caught components with a
`pdf geometry [` refusal. Also: a self-test that parsed needles with a single-quote sed skipped a
double-quoted fifth needle and still printed OK.

**How to apply:** for any payload-absence gate, plant one export per component the ADR names,
build the real module, swap it into a production-shaped dist copy, run the real checker, and
record size added and needles hit per component. Check the self-test reads its rule list from
the checker (a print mode), not by parsing it. The gate now carries this as a `--cfg
burrow_redaction_probe` built positive. Related: [[feedback_residual_bound_by_uncounted_dimension]].
