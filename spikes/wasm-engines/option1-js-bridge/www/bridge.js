// The JS bridge for option 1.
//
// Deliberately mechanical: allocate in the engine's heap, copy the bytes in, call one
// function, free. Every decision -- what an error code means, which engine to try, what
// to do next -- stays in Rust. That is the discipline ADR 0002 asks for, and keeping to
// it here is part of what the spike is evaluating.
//
// Note what "copy the bytes in" costs: the file already exists in the Rust module's
// heap, and this puts a second copy in the engine's heap. Two wasm modules cannot share
// linear memory, so this is inherent to the option, not an implementation shortcut.

let pdfium = null;
let qpdf = null;

async function initEngines(baseUrl) {
  // PDFium is NOT modularised: its `var Module = typeof Module != "undefined" ? ... : {}`
  // must run at global scope for our config to survive, which importScripts() gives us.
  if (!pdfium) {
    self.Module = { locateFile: (f) => `${baseUrl}/pdfium/${f}` };
    self.importScripts(`${baseUrl}/pdfium/pdfium.js`);
    pdfium = await new Promise((resolve) => {
      const m = self.Module;
      if (m.calledRun) resolve(m);
      else m.onRuntimeInitialized = () => resolve(m);
    });
    pdfium._FPDF_InitLibrary();
  }
  // qpdf we built ourselves, so it is modularised and behaves.
  if (!qpdf) {
    self.importScripts(`${baseUrl}/qpdf/qpdf.js`);
    qpdf = await self.createQpdf({ locateFile: (f) => `${baseUrl}/qpdf/${f}` });
  }
  return true;
}

function withEngineHeap(mod, bytes, fn) {
  const ptr = mod._malloc(bytes.length);
  if (!ptr) throw new Error("engine malloc failed");
  try {
    mod.HEAPU8.set(bytes, ptr);
    return fn(ptr, bytes.length);
  } finally {
    mod._free(ptr);
  }
}

function pdfiumPageCount(bytes) {
  return withEngineHeap(pdfium, bytes, (ptr, len) => {
    const doc = pdfium._FPDF_LoadMemDocument(ptr, len, 0);
    if (!doc) return -pdfium._FPDF_GetLastError(); // negative sentinel; Rust types it
    try {
      return pdfium._FPDF_GetPageCount(doc);
    } finally {
      pdfium._FPDF_CloseDocument(doc);
    }
  });
}

function qpdfPageCount(bytes) {
  return withEngineHeap(qpdf, bytes, (ptr, len) => qpdf._qpdf_probe_pages(ptr, len));
}

// Peak heap of both engine modules, for the memory measurement.
function engineHeapBytes() {
  return (pdfium?.HEAPU8?.length ?? 0) + (qpdf?.HEAPU8?.length ?? 0);
}

// Classic script, so publish on the global scope for wasm-bindgen's imports to resolve.
self.initEngines = initEngines;
self.pdfiumPageCount = pdfiumPageCount;
self.qpdfPageCount = qpdfPageCount;
self.engineHeapBytes = engineHeapBytes;
