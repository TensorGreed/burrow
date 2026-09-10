// Node probe: does the prebuilt PDFium module open a PDF and report a page count?
// Node first because the feedback loop is seconds. The real bar is the browser worker
// (see worker.js + the Playwright test); this only de-risks the FPDF call sequence.
import { readFileSync } from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { createRequire } from "node:module";

const LIB = path.resolve(import.meta.dirname, "../vendor/pdfium/lib");
const CORPUS = path.resolve(import.meta.dirname, "../common/corpus");

// The module is NOT modularised: its first statement is
//   var Module = typeof Module != "undefined" ? Module : {}
// Under CommonJS `require` that runs at *function* scope, where the hoisted local
// `Module` shadows any global we set, so our config is silently discarded and the
// init hook never fires. Run it at true global scope instead: there, `var` over an
// existing global property is a no-op, so our object survives.
//
// A browser worker gets this for free -- importScripts() is global scope. This
// workaround is Node-only.
// The node branch of the glue uses `require` and `__dirname`, which do not exist at
// ESM/vm global scope. Supply them.
globalThis.require = createRequire(import.meta.url);
globalThis.__dirname = LIB;

const mod = await new Promise((resolve, reject) => {
  globalThis.Module = {
    locateFile: (f) => path.join(LIB, f),
    onRuntimeInitialized: () => resolve(globalThis.Module),
    onAbort: (w) => reject(new Error(`pdfium abort: ${w}`)),
  };
  vm.runInThisContext(readFileSync(path.join(LIB, "pdfium.js"), "utf8"), {
    filename: "pdfium.js",
  });
});

const c = (name, ret, args) => mod.cwrap(name, ret, args);
const FPDF_InitLibrary = c("FPDF_InitLibrary", null, []);
const FPDF_LoadMemDocument = c("FPDF_LoadMemDocument", "number", ["number", "number", "string"]);
const FPDF_GetPageCount = c("FPDF_GetPageCount", "number", ["number"]);
const FPDF_CloseDocument = c("FPDF_CloseDocument", null, ["number"]);
const FPDF_GetLastError = c("FPDF_GetLastError", "number", []);

FPDF_InitLibrary();

const ERRORS = {
  0: "SUCCESS", 1: "UNKNOWN", 2: "FILE", 3: "FORMAT",
  4: "PASSWORD", 5: "SECURITY", 6: "PAGE",
};

function pageCount(file) {
  const bytes = readFileSync(path.join(CORPUS, file));
  const ptr = mod._malloc(bytes.length);
  mod.HEAPU8.set(bytes, ptr);
  const t0 = performance.now();
  const doc = FPDF_LoadMemDocument(ptr, bytes.length, null);
  let out;
  if (!doc) {
    out = { ok: false, err: ERRORS[FPDF_GetLastError()] ?? FPDF_GetLastError() };
  } else {
    out = { ok: true, pages: FPDF_GetPageCount(doc) };
    FPDF_CloseDocument(doc);
  }
  out.ms = +(performance.now() - t0).toFixed(1);
  out.bytes = bytes.length;
  mod._free(ptr);
  return out;
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
    r = pageCount(f);
  } catch (e) {
    r = { ok: false, threw: String(e).slice(0, 80) };
  }
  console.log(`  ${f.padEnd(24)} ${JSON.stringify(r)}`);
}
console.log("\n  heap after all opens:", (mod.HEAPU8.length / 1e6).toFixed(1), "MB");
