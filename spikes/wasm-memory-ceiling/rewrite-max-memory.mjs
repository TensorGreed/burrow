#!/usr/bin/env node
// Rewrite the declared MAXIMUM memory in a WebAssembly module's memory section, in place,
// changing nothing else.
//
// SPIKE 0002. Throwaway. See docs/spikes/0002-wasm-memory-ceiling.md.
//
// WHY A TARGETED PATCH RATHER THAN A TOOLKIT
//
// `wasm-tools`, `binaryen` and `wabt` can all do this by decoding the module and re-encoding
// it. Rejected for this spike, on two grounds:
//
//   1. **Auditability.** A re-encode rewrites the whole file, so the diff against the upstream
//      artifact is every byte, and "we changed the memory limit and nothing else" stops being
//      checkable. A targeted patch changes one or two bytes at a computed offset and leaves
//      the length identical, so `cmp` is the audit. `--verify` below turns that into an
//      assertion rather than a claim.
//   2. **Licence surface.** Adding a large dependency to edit two bytes would need an ADR 0008
//      admission-test pass. This file has no dependencies at all: Node's standard library and
//      about a hundred lines of LEB128.
//
// HOW THE PATCH KEEPS EVERY OTHER OFFSET INTACT
//
// A memory type is `flags:u8, min:varu32 [, max:varu32]`. Both engines encode
// max = 32768 pages (2 GiB) as the 3-byte LEB128 `80 80 02`. A smaller number needs fewer
// bytes in its minimal form -- which would shrink the section and shift everything after it,
// including the section's own size varint.
//
// LEB128 permits NON-MINIMAL encodings within the type's byte bound (u32 -> at most 5 bytes),
// so the new value is written PADDED to exactly the width the old one occupied. Nothing moves,
// the file length is unchanged, and the patch is visible as one or two differing bytes.
//
// Usage:
//   rewrite-max-memory.mjs <file.wasm> --max-mib <n> [--out <file>] [--verify <original>]
//   rewrite-max-memory.mjs <file.wasm> --report

import { readFileSync, writeFileSync } from "node:fs";

const PAGE_BYTES = 65536;

/** Read an unsigned LEB128 at `i`. Returns `{ value, next, width }`. */
function readVaruint(bytes, i) {
  let value = 0;
  let shift = 0;
  const start = i;
  for (;;) {
    if (i >= bytes.length) throw new Error(`truncated LEB128 at offset ${start}`);
    const byte = bytes[i++];
    value += (byte & 0x7f) * 2 ** shift;
    shift += 7;
    if ((byte & 0x80) === 0) break;
    if (shift > 35) throw new Error(`LEB128 at offset ${start} exceeds u32`);
  }
  return { value, next: i, width: i - start };
}

/**
 * Encode `value` as LEB128 padded to exactly `width` bytes.
 *
 * Non-minimal, deliberately: see the header. Throws if `value` does not fit, which is the
 * case that would otherwise silently shift the rest of the file.
 */
function encodeVaruintPadded(value, width) {
  if (width < 1 || width > 5) throw new Error(`unsupported LEB128 width ${width}`);
  const out = Buffer.alloc(width);
  let v = value;
  for (let i = 0; i < width; i += 1) {
    out[i] = (v & 0x7f) | (i < width - 1 ? 0x80 : 0);
    v = Math.floor(v / 128);
  }
  if (v !== 0) {
    // UNREACHABLE through `rewriteMaxPages`, which refuses to raise the maximum -- and a value
    // at or below the current one always fits the width that one occupies, since LEB128 width
    // grows with magnitude. Kept for a future direct caller of this function; the invariant is
    // asserted in test-rewrite-max-memory.mjs rather than left as reasoning.
    throw new Error(
      `${value} does not fit in ${width} LEB128 byte(s); a wider encoding would shift the ` +
        `rest of the module, which this tool refuses to do`,
    );
  }
  return out;
}

/** Locate the memory section's limits. Returns the offsets a patch would touch. */
export function findMemoryLimits(bytes) {
  if (bytes.length < 8 || bytes.readUInt32BE(0) !== 0x0061736d) {
    throw new Error("not a WebAssembly module (bad magic)");
  }

  let i = 8;
  while (i < bytes.length) {
    const id = bytes[i++];
    const { value: size, next } = readVaruint(bytes, i);
    i = next;
    const sectionStart = i;

    if (id === 5) {
      let j = sectionStart;
      const count = readVaruint(bytes, j);
      j = count.next;
      if (count.value !== 1) {
        throw new Error(`expected exactly 1 memory, found ${count.value}`);
      }
      const flags = bytes[j];
      j += 1;
      const min = readVaruint(bytes, j);
      j = min.next;
      if ((flags & 0x01) === 0) {
        throw new Error("memory declares no maximum; there is nothing to lower");
      }
      const max = readVaruint(bytes, j);
      return {
        sectionOffset: sectionStart,
        sectionSize: size,
        flags,
        minPages: min.value,
        maxOffset: j,
        maxWidth: max.width,
        maxPages: max.value,
      };
    }
    i += size;
  }
  throw new Error("no memory section (id 5) in this module");
}

/** Patch the maximum to `pages`, returning the new bytes. Does not write anything. */
export function rewriteMaxPages(bytes, pages) {
  const limits = findMemoryLimits(bytes);
  if (pages > limits.maxPages) {
    throw new Error(
      `refusing to RAISE the maximum: module declares ${limits.maxPages} pages, asked for ` +
        `${pages}. This spike only lowers it; raising would weaken a bound, not add one`,
    );
  }
  if (pages < limits.minPages) {
    throw new Error(
      `maximum ${pages} pages is below the module's declared minimum ${limits.minPages}; ` +
        `the module could never instantiate`,
    );
  }
  const patched = Buffer.from(bytes);
  encodeVaruintPadded(pages, limits.maxWidth).copy(patched, limits.maxOffset);
  return { patched, limits };
}

/**
 * Every offset at which two buffers differ.
 *
 * The whole "targeted patch, not a re-encode" argument rests on this being a short list, so
 * it is computed and asserted rather than asserted in prose.
 */
export function differingOffsets(a, b) {
  const out = [];
  const n = Math.max(a.length, b.length);
  for (let i = 0; i < n; i += 1) {
    if (a[i] !== b[i]) out.push(i);
  }
  return out;
}

const mib = (pages) => (pages * PAGE_BYTES) / (1024 * 1024);

function main(argv) {
  const file = argv[0];
  if (!file || file.startsWith("--")) {
    console.error(
      "usage: rewrite-max-memory.mjs <file.wasm> --max-mib <n> [--out <f>] [--verify <orig>]\n" +
        "       rewrite-max-memory.mjs <file.wasm> --report",
    );
    return 2;
  }
  const arg = (name) => {
    const i = argv.indexOf(name);
    return i === -1 ? null : argv[i + 1];
  };

  const original = readFileSync(file);

  if (argv.includes("--report")) {
    const l = findMemoryLimits(original);
    console.log(`${file}`);
    console.log(`  size            ${original.length} bytes`);
    console.log(`  memory section  offset 0x${l.sectionOffset.toString(16)}, ${l.sectionSize} bytes`);
    console.log(`  flags           0x${l.flags.toString(16).padStart(2, "0")}`);
    console.log(`  initial         ${l.minPages} pages (${mib(l.minPages)} MiB)`);
    console.log(
      `  maximum         ${l.maxPages} pages (${mib(l.maxPages)} MiB), ` +
        `${l.maxWidth} LEB128 bytes at 0x${l.maxOffset.toString(16)}: ` +
        `${original.subarray(l.maxOffset, l.maxOffset + l.maxWidth).toString("hex").replace(/../g, "$& ").trim()}`,
    );
    return 0;
  }

  const maxMib = Number(arg("--max-mib"));
  if (!Number.isInteger(maxMib) || maxMib <= 0) {
    console.error("error: --max-mib must be a positive integer");
    return 2;
  }
  const pages = (maxMib * 1024 * 1024) / PAGE_BYTES;
  if (!Number.isInteger(pages)) {
    console.error(`error: ${maxMib} MiB is not a whole number of 64 KiB pages`);
    return 2;
  }

  const { patched, limits } = rewriteMaxPages(original, pages);
  const changed = differingOffsets(original, patched);
  const expected = new Set(
    Array.from({ length: limits.maxWidth }, (_, k) => limits.maxOffset + k),
  );

  // GATES, not observations. Each is a way the patch could be wrong that would otherwise
  // produce a plausible-looking file.
  const problems = [];
  if (patched.length !== original.length) {
    problems.push(`length changed: ${original.length} -> ${patched.length}`);
  }
  const stray = changed.filter((o) => !expected.has(o));
  if (stray.length) {
    problems.push(
      `bytes changed outside the maximum field: ${stray.map((o) => `0x${o.toString(16)}`).join(", ")}`,
    );
  }
  if (changed.length === 0) {
    problems.push(`no bytes changed; ${maxMib} MiB is already the declared maximum`);
  }
  try {
    new WebAssembly.Module(patched);
  } catch (error) {
    problems.push(`the patched module does not compile: ${error.message}`);
  }

  // COVERAGE REPORT, per CLAUDE.md: what was examined, not just a verdict.
  console.log(`${file}`);
  console.log(`  maximum   ${limits.maxPages} pages (${mib(limits.maxPages)} MiB)` +
    ` -> ${pages} pages (${maxMib} MiB)`);
  console.log(`  field     ${limits.maxWidth} bytes at 0x${limits.maxOffset.toString(16)}: ` +
    `${original.subarray(limits.maxOffset, limits.maxOffset + limits.maxWidth).toString("hex")}` +
    ` -> ${patched.subarray(limits.maxOffset, limits.maxOffset + limits.maxWidth).toString("hex")}`);
  console.log(`  changed   ${changed.length} byte(s), all within the maximum field`);
  console.log(`  size      ${original.length} bytes, unchanged`);
  console.log(`  compiles  ${problems.some((p) => p.includes("does not compile")) ? "NO" : "yes"}`);

  if (arg("--verify")) {
    const reference = readFileSync(arg("--verify"));
    const vs = differingOffsets(reference, patched);
    const vsStray = vs.filter((o) => !expected.has(o));
    console.log(
      `  vs ${arg("--verify")}: ${vs.length} differing byte(s), ` +
        `${vsStray.length} outside the maximum field`,
    );
    if (vsStray.length) {
      problems.push(
        `differs from the reference outside the maximum field at ` +
          `${vsStray.slice(0, 8).map((o) => `0x${o.toString(16)}`).join(", ")}` +
          `${vsStray.length > 8 ? ` (+${vsStray.length - 8} more)` : ""}`,
      );
    }
  }

  if (problems.length) {
    console.error(`\nFAILED — ${problems.length} problem(s):`);
    for (const p of problems) console.error(`  - ${p}`);
    return 1;
  }

  const out = arg("--out") ?? file;
  writeFileSync(out, patched);
  console.log(`  wrote     ${out}`);
  return 0;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  process.exit(main(process.argv.slice(2)));
}
