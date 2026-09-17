/**
 * Every field the worker reads off a `Reply` is a name the Rust actually exposes.
 *
 * # The failure this exists to prevent, which happened
 *
 * `compress` added two counts to `Reply`. The Rust getters were written as
 *
 * ```rust
 * #[wasm_bindgen(getter)]
 * pub fn original_bytes(&self) -> u64
 * ```
 *
 * and **wasm-bindgen does not camelCase a getter** — it uses the Rust name unless `js_name`
 * says otherwise, which is why `failed_input`, `inner_kind` and `output_length` all carry one
 * and `engine_heap_bytes` deliberately does not. So the module exposed `original_bytes`, while
 * `drainReply` read `reply.originalBytes`.
 *
 * That is `undefined`, `drainReply` calls `.toString()` on it, and the guard around
 * `drainReply` turns the throw into a typed `Internal` — so **every operation through the
 * worker failed**, and the first sign was a wall of red in the Playwright suite with an error
 * that named nothing.
 *
 * # Why nothing caught it
 *
 * Three things agreed with each other and all three were wrong about the module:
 *
 * - `drainReply`'s read, `reply.originalBytes`;
 * - `src/worker/test-scope.ts`'s shared reply fake, whose comment says *"Field names match the
 *   wasm-bindgen getters"* — a claim nothing checked;
 * - `src/worker/globals.d.ts`, hand-written to the same assumption.
 *
 * The unit suites passed because the fake matched the reader. A stub cannot disagree with the
 * code it was written beside; only the real declaration can.
 *
 * # What this compares, and why it is these two files
 *
 * The **Rust source** and the **worker source**, both committed — so it runs on a clean
 * checkout with no build, no `pkg/`, and no browser. The generated glue would be the more
 * direct witness, but it is gitignored, per-machine and minified, and parsing a minified class
 * to prove a naming rule is a second thing to get wrong.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const repo = join(import.meta.dirname, "..", "..", "..");
const rustPath = join(repo, "bindings", "burrow-wasm", "src", "lib.rs");
/**
 * EVERY WORKER SOURCE THAT READS A REPLY, not just the one that used to.
 *
 * `main.js` held the whole protocol until ADR 0026 split the worker into two bundles;
 * `drainReply` — which is where every `reply.X` read lives — moved to `worker-protocol.js`,
 * and `render-main.js` is the second bundle's dispatch. Reading only the old path would have
 * left this check scanning a file with almost no reads in it and reporting a clean run, which
 * is exactly the vacuous-pass shape it was written to prevent.
 *
 * The floor assertion below is what turns that from a hope into a failure: a set of reads
 * smaller than eleven fails, so losing a source file is loud.
 */
const workerPaths = ["worker-protocol.js", "main.js", "render-main.js"].map((file) =>
  join(repo, "apps", "web", "src", "worker", file),
);

/**
 * Every name a `Reply` exposes to JavaScript.
 *
 * A `#[wasm_bindgen(...)]` attribute carrying `js_name = X` exposes `X`; one without exposes
 * the Rust function's own name, unchanged. That is the whole rule, and it is the rule the
 * defect broke.
 */
function exposedNames(rust: string): Set<string> {
  const names = new Set<string>();
  const pattern = /#\[wasm_bindgen\(([^)]*)\)\]\s*(?:#\[[^\]]*\]\s*)*pub fn ([a-z_][a-z0-9_]*)/g;
  for (const [, attrs, fn] of rust.matchAll(pattern)) {
    const renamed = /js_name\s*=\s*([A-Za-z_][A-Za-z0-9_]*)/.exec(attrs);
    names.add(renamed ? renamed[1] : fn);
  }
  return names;
}

/** Every `reply.X` the worker reads. */
function readNames(worker: string): Set<string> {
  // COMMENTS ARE STRIPPED FIRST, and that is not tidiness. The first version of this check
  // scanned the whole file and reported `reply.output` missing -- a name that appears only in
  // a doc comment explaining that the terminal reply of a multi-output operation carries no
  // bytes. A check that fires on prose is a check somebody turns off.
  //
  // Dropped by first non-space character rather than by matching `//` anywhere: a line with a
  // URL in a string would otherwise lose its tail, which would hide a real read. The residue
  // is a trailing comment after code on the same line, which is still scanned -- the direction
  // that over-reports rather than under-reports.
  const code = worker
    .split("\n")
    .filter((line) => {
      const trimmed = line.trimStart();
      return !(trimmed.startsWith("//") || trimmed.startsWith("*") || trimmed.startsWith("/*"));
    })
    .join("\n");
  return new Set([...code.matchAll(/\breply\.([A-Za-z_][A-Za-z0-9_]*)/g)].map((m) => m[1]));
}

describe("the worker reads only names the Rust exposes", () => {
  const rust = readFileSync(rustPath, "utf8");
  const worker = workerPaths.map((path) => readFileSync(path, "utf8")).join("\n");

  it("finds both halves, so a silent parse failure cannot pass", () => {
    // THE PROBE. Two empty sets compare equal, and a regex that stopped matching would report
    // a clean run over nothing at all — which is the shape this repository keeps being caught
    // by. The counts are floors rather than exact numbers: the point is that the extraction
    // works, not that it froze on today's field list.
    const exposed = exposedNames(rust);
    const reads = readNames(worker);
    expect(exposed.size).toBeGreaterThan(10);
    expect(reads.size).toBeGreaterThan(10);

    // AND IT RECOGNISES BOTH SPELLINGS, which is the distinction the defect turned on.
    expect(exposed).toContain("failedInput"); // renamed by js_name
    expect(exposed).toContain("engine_heap_bytes"); // deliberately not renamed
  });

  it("every field the worker reads is exposed under that name", () => {
    const exposed = exposedNames(rust);
    // `free` is wasm-bindgen's own, generated for every exported struct rather than declared.
    const generated = new Set(["free"]);
    const missing = [...readNames(worker)].filter(
      (name) => !exposed.has(name) && !generated.has(name),
    );
    expect(
      missing,
      "the worker reads a Reply field the Rust does not expose under that name. " +
        "wasm-bindgen does NOT camelCase a getter: add `js_name = ...` to the attribute, or " +
        "read the Rust name. Left unfixed this is `undefined` at run time, and the guard " +
        "around drainReply turns it into an Internal for EVERY operation.",
    ).toEqual([]);
  });

  it("the check fails when a read has no matching name", () => {
    // THE NEAR-MISS, planted rather than trusted: the assertion above passing is only worth
    // something if it can fail. This is the exact defect, in a copy.
    const exposed = exposedNames(rust);
    const planted = readNames(`${worker}\n const x = reply.originalBytesTypo;`);
    const missing = [...planted].filter((name) => !exposed.has(name) && name !== "free");
    expect(missing).toEqual(["originalBytesTypo"]);
  });

  it("the check fails when a getter loses its js_name", () => {
    // THE DEFECT ITSELF, from the other direction: strip `js_name = originalBytes` and the
    // worker's read stops being covered. If this ever passes, the rename rule is no longer
    // what this file says it is.
    const stripped = rust.replace(
      "#[wasm_bindgen(getter, js_name = originalBytes)]",
      "#[wasm_bindgen(getter)]",
    );
    expect(stripped, "the mutation did not apply").not.toEqual(rust);

    const exposed = exposedNames(stripped);
    expect(exposed.has("originalBytes")).toBe(false);
    expect(exposed.has("original_bytes")).toBe(true);

    const missing = [...readNames(worker)].filter((name) => !exposed.has(name) && name !== "free");
    expect(missing).toEqual(["originalBytes"]);
  });
});
