#!/usr/bin/env node
// Adversarial self-test for rewrite-max-memory.mjs.
//
// SPIKE 0002. The tool's whole argument over a decode-and-re-encode toolkit is "it changes
// only the maximum field and nothing else". That is a property, so it is asserted here rather
// than claimed in the header:
//
//   * a FULL byte comparison against the upstream original, failing on any difference outside
//     the expected offsets -- not a length check, not a spot check;
//   * refusal on every input shape that would otherwise produce a plausible-looking file.
//
// Every mutation-style fixture asserts the mutation applied before the case runs, per
// CLAUDE.md: a fixture that silently failed to mutate is indistinguishable from a defence
// that works.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { differingOffsets, findMemoryLimits, rewriteMaxPages } from "./rewrite-max-memory.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const ENGINES = [
  join(repo, "engines/vendor/wasm/lib/pdfium.wasm"),
  join(repo, "engines/vendor/wasm/lib/qpdf.wasm"),
];

let pass = 0;
const failures = [];

function check(name, fn) {
  try {
    fn();
    console.log(`  ok   ${name}`);
    pass += 1;
  } catch (error) {
    console.log(`  FAIL ${name}: ${error.message}`);
    failures.push(name);
  }
}

function refuses(name, fn, expectedFragment) {
  check(name, () => {
    let threw = null;
    try {
      fn();
    } catch (error) {
      threw = error;
    }
    if (!threw) throw new Error("it accepted the input instead of refusing it");
    if (!threw.message.includes(expectedFragment)) {
      throw new Error(`refused for the wrong reason: ${threw.message}`);
    }
  });
}

// A minimal hand-built module, so the shape cases do not depend on the vendor tree.
function tinyWasm({ withMemory = true, withMax = true, memories = 1 } = {}) {
  const parts = [Buffer.from([0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00])];
  if (withMemory) {
    const entries = [];
    for (let i = 0; i < memories; i += 1) {
      entries.push(withMax ? Buffer.from([0x01, 0x01, 0x02]) : Buffer.from([0x00, 0x01]));
    }
    const body = Buffer.concat([Buffer.from([memories]), ...entries]);
    parts.push(Buffer.from([0x05, body.length]), body);
  }
  return Buffer.concat(parts);
}

console.log("fixtures: the two staged engines, plus hand-built modules for the shape cases\n");

for (const path of ENGINES) {
  const name = path.split("/").pop();
  const original = readFileSync(path);
  const limits = findMemoryLimits(original);
  const expected = new Set(
    Array.from({ length: limits.maxWidth }, (_, k) => limits.maxOffset + k),
  );

  // THE CENTRAL ASSERTION. Full byte comparison against the upstream original, at every
  // ceiling the spike sweeps. Anything differing outside the maximum field is a failure.
  check(`${name}: only the maximum field differs, at every swept ceiling`, () => {
    for (const targetMib of [1024, 512, 256, 128, 64]) {
      const pages = (targetMib * 1024 * 1024) / 65536;
      const { patched } = rewriteMaxPages(original, pages);
      const stray = differingOffsets(original, patched).filter((o) => !expected.has(o));
      if (stray.length) {
        throw new Error(
          `${targetMib} MiB changed ${stray.length} byte(s) outside the maximum field: ` +
            stray.slice(0, 8).map((o) => `0x${o.toString(16)}`).join(", "),
        );
      }
      if (patched.length !== original.length) {
        throw new Error(`${targetMib} MiB changed the file length`);
      }
      new WebAssembly.Module(patched); // throws if invalid
    }
  });

  check(`${name}: the patched value reads back as the value we asked for`, () => {
    for (const targetMib of [1024, 512, 256, 128, 64]) {
      const pages = (targetMib * 1024 * 1024) / 65536;
      const { patched } = rewriteMaxPages(original, pages);
      const after = findMemoryLimits(patched);
      if (after.maxPages !== pages) {
        throw new Error(`asked for ${pages} pages, module now declares ${after.maxPages}`);
      }
      if (after.minPages !== limits.minPages) {
        throw new Error(`the initial memory moved: ${limits.minPages} -> ${after.minPages}`);
      }
    }
  });

  refuses(
    `${name}: refuses to RAISE the maximum`,
    () => rewriteMaxPages(original, limits.maxPages + 1024),
    "refusing to RAISE",
  );

  refuses(
    `${name}: refuses a maximum below the declared initial`,
    () => rewriteMaxPages(original, limits.minPages - 1),
    "below the module's declared minimum",
  );
}

// --- shape cases, on hand-built modules -------------------------------------------------

refuses(
  "refuses a non-wasm input",
  () => findMemoryLimits(Buffer.from("this is definitely not a wasm module")),
  "bad magic",
);

refuses(
  "refuses a module with no memory section",
  () => findMemoryLimits(tinyWasm({ withMemory: false })),
  "no memory section",
);

refuses(
  "refuses a memory that declares no maximum",
  () => findMemoryLimits(tinyWasm({ withMax: false })),
  "declares no maximum",
);

refuses(
  "refuses a module with more than one memory",
  () => findMemoryLimits(tinyWasm({ memories: 2 })),
  "expected exactly 1 memory",
);

// LOWERING CAN NEVER NEED A WIDER ENCODING, and that is what makes the whole approach safe.
//
// This case started out as "refuses a value too wide for the existing field". It could not be
// written: LEB128 width grows with magnitude, so a value below the current maximum always fits
// the width the maximum already occupies. The `does not fit` guard in `encodeVaruintPadded` is
// therefore unreachable through `rewriteMaxPages` -- the RAISE guard fires first -- and is kept
// only for a future direct caller of the exported function.
//
// So the property worth asserting is the invariant itself, exhaustively over the real fields:
// every page count from 1 to the declared maximum encodes within the existing width. If that
// ever stopped holding, the tool would have to shift the module, which it refuses to do.
check("every value at or below the maximum fits the existing field width", () => {
  for (const path of ENGINES) {
    const original = readFileSync(path);
    const { maxPages, maxWidth, minPages } = findMemoryLimits(original);
    const capacity = 2 ** (7 * maxWidth);
    if (maxPages >= capacity) {
      throw new Error(
        `${path}: the declared maximum ${maxPages} does not itself fit ${maxWidth} bytes`,
      );
    }
    // Only LEGAL targets: at or above the declared initial, at or below the declared maximum.
    // Below the initial is refused for a different and correct reason (the module could never
    // instantiate), which is its own case above.
    const probes = [minPages, maxPages];
    for (let v = 1; v < maxPages; v *= 2) if (v >= minPages) probes.push(v);
    for (const pages of probes) {
      const { patched } = rewriteMaxPages(original, pages);
      if (patched.length !== original.length) {
        throw new Error(`${path}: ${pages} pages changed the file length`);
      }
    }
  }
});

check("a corrupted maximum field is detected by the byte comparison", () => {
  // The negative control for the central assertion: if the tool ever touched a byte outside
  // the field, `differingOffsets` must see it. Planted directly, so the comparison itself is
  // exercised rather than trusted.
  const original = readFileSync(ENGINES[0]);
  const limits = findMemoryLimits(original);
  const tampered = Buffer.from(original);
  const strayOffset = limits.maxOffset + limits.maxWidth + 64;
  tampered[strayOffset] ^= 0xff;
  const expected = new Set(
    Array.from({ length: limits.maxWidth }, (_, k) => limits.maxOffset + k),
  );
  const stray = differingOffsets(original, tampered).filter((o) => !expected.has(o));
  if (!stray.includes(strayOffset)) {
    throw new Error("the byte comparison missed a planted difference outside the field");
  }
});

console.log();
if (failures.length) {
  console.error(`FAILED — ${failures.length} case(s) failed, ${pass} passed`);
  process.exit(1);
}
console.log(`OK — ${pass} adversarial case(s) all behaved as required`);
