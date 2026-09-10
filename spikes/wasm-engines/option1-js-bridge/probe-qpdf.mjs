// Does a malformed PDF come back from qpdf as a typed error, or does it abort?
// This is the C++ exceptions test that ADR 0006 says decides the linking strategy.
import { readFileSync } from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const CORPUS = path.resolve(import.meta.dirname, "../common/corpus");
// Modularised, so a plain require works -- no global-scope workaround needed.
const createQpdf = require(path.resolve(import.meta.dirname, "../build/qpdf.js"));

const mod = await createQpdf({});
const pages = mod.cwrap("qpdf_probe_pages", "number", ["number", "number"]);
const lastMsg = mod.cwrap("qpdf_probe_last_message", "string", []);

const STATUS = { 0: "OK", "-1": "MALFORMED", "-2": "PASSWORD_REQUIRED", "-3": "UNSUPPORTED", "-4": "INTERNAL" };

function probe(file) {
  const bytes = readFileSync(path.join(CORPUS, file));
  const ptr = mod._malloc(bytes.length);
  mod.HEAPU8.set(bytes, ptr);
  const t0 = performance.now();
  const rc = pages(ptr, bytes.length);
  const ms = +(performance.now() - t0).toFixed(1);
  mod._free(ptr);
  return rc >= 0
    ? { pages: rc, ms }
    : { status: STATUS[String(rc)] ?? rc, ms, msg: lastMsg().slice(0, 60) };
}

for (const f of [
  "small-1page.pdf",
  "medium-100page.pdf",
  "large-50mb.pdf",
  "malformed-truncated.pdf",
  "malformed-badxref.pdf",
  "malformed-notpdf.bin",
]) {
  let r;
  try {
    r = probe(f);
  } catch (e) {
    r = { ABORTED: String(e).slice(0, 90) };
  }
  console.log(`  ${f.padEnd(24)} ${JSON.stringify(r)}`);
}
console.log("\n  still alive after all of the above:", mod._qpdf_probe_version_ok() === 1);
