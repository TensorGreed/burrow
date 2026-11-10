// A full scan of every WebAssembly linear memory the RENDER worker instantiates, for named byte
// patterns -- the PDFium half of ADR 0031 section 7's heap canary.
//
// NO PRODUCT CODE, NO HARNESS CHANGE. The harness driver already prepends a prologue to the
// redaction worker's source (`heapScanPrologue`); the render worker has no such hook, and adding
// one would touch an existing file on the worker boundary. This does the same thing from the
// test side instead: an init script, run in the page before anything else, wraps
// `URL.createObjectURL` so a JavaScript `Blob` (the integrity-checked bundle the harness turns
// into a worker) gets the prologue below prepended, and wraps `Worker` so the page can send the
// worker a scan request and read its answer BEFORE the host's own listeners see either.
//
// HARNESS-ONLY: nothing here is reachable from a production build, which has no harness page and
// installs no init script.
//
// WHICH MEMORY IS WHICH. Every memory the worker instantiates is recorded (exported or imported).
// An instance that exports a `FPDF_*` / `_FPDF_*` symbol is labelled `pdfium`; one that exports a
// `__wbindgen_*` symbol is labelled `burrow-render`. A scan lists each memory it examined, with
// its size, so a capture that missed one says so rather than reading as clean.
//
// PROLOGUE LABEL: the wrapper matches EVERY `text/javascript` Blob URL, not just the render
// bundle. That is fine while the render worker is the only such worker; the scan then asks each
// wrapped worker and keeps the one whose memories include both labels. Before another blob-URL
// worker is added (S12), match the render bundle explicitly.

import type { Page } from "@playwright/test";

export interface Needle {
  name: string;
  /** The bytes to find. */
  bytes: number[];
}

export interface MemoryScan {
  heap: string;
  bytes: number;
  /** Non-overlapping occurrences per needle name. */
  hits: Record<string, number>;
}

/** Text as the bytes a content stream holds it in. */
export function textNeedle(name: string, text: string): Needle {
  return { name, bytes: Array.from(Buffer.from(text, "latin1")) };
}

/** `count` identical 4-byte pixels, in the given byte order. */
export function pixelNeedle(name: string, pixel: number[], count: number): Needle {
  return { name, bytes: Array.from({ length: count }, () => pixel).flat() };
}

const PROLOGUE = `
(() => {
  const memories = [];
  const labels = new Map();
  const label = (exports) => {
    const names = Object.keys(exports || {});
    if (names.some((n) => /^_?FPDF/.test(n))) return "pdfium";
    if (names.some((n) => n.startsWith("__wbindgen"))) return "burrow-render";
    return null;
  };
  const keep = (value, tag) => {
    if (!(value instanceof WebAssembly.Memory)) return;
    if (!memories.includes(value)) memories.push(value);
    if (tag && !labels.has(value)) labels.set(value, tag);
  };
  const fromImports = (imports) => {
    for (const space of Object.values(imports || {})) {
      for (const value of Object.values(space || {})) keep(value, null);
    }
  };
  const fromInstance = (instance, imports) => {
    const tag = label(instance.exports);
    for (const value of Object.values(instance.exports || {})) keep(value, tag);
    for (const space of Object.values(imports || {})) {
      for (const value of Object.values(space || {})) keep(value, tag);
    }
  };
  const instantiate = WebAssembly.instantiate.bind(WebAssembly);
  WebAssembly.instantiate = async (source, imports) => {
    fromImports(imports);
    const made = await instantiate(source, imports);
    fromInstance(made.instance || made, imports);
    return made;
  };
  if (WebAssembly.instantiateStreaming) {
    const streaming = WebAssembly.instantiateStreaming.bind(WebAssembly);
    WebAssembly.instantiateStreaming = async (source, imports) => {
      fromImports(imports);
      const made = await streaming(source, imports);
      fromInstance(made.instance, imports);
      return made;
    };
  }
  const Instance = WebAssembly.Instance;
  WebAssembly.Instance = function (module, imports) {
    fromImports(imports);
    const made = new Instance(module, imports);
    fromInstance(made, imports);
    return made;
  };
  WebAssembly.Instance.prototype = Instance.prototype;

  const count = (heap, needle) => {
    let hits = 0;
    const first = needle[0];
    let at = heap.indexOf(first);
    while (at !== -1 && at <= heap.length - needle.length) {
      let match = true;
      for (let i = 1; i < needle.length; i++) {
        if (heap[at + i] !== needle[i]) { match = false; break; }
      }
      if (match) { hits++; at = heap.indexOf(first, at + needle.length); }
      else at = heap.indexOf(first, at + 1);
    }
    return hits;
  };
  const post = self.postMessage.bind(self);
  // CAPTURE, first: nothing the bundle registers later sees the request (same as the settle
  // handshake in harness-driver.js).
  self.addEventListener("message", (event) => {
    const request = event.data && event.data.__burrowScan;
    if (!request) return;
    event.stopImmediatePropagation();
    const result = memories.map((memory) => {
      const heap = new Uint8Array(memory.buffer);
      const hits = {};
      for (const needle of request.needles) hits[needle.name] = count(heap, Uint8Array.from(needle.bytes));
      return { heap: labels.get(memory) || "unlabelled", bytes: heap.length, hits };
    });
    post({ __burrowScanned: request.nonce, result });
  }, { capture: true });
})();
`;

/**
 * Install the scanner. Call BEFORE `openHarness`, so the init script precedes the page's own.
 * The scan is read with {@link scanRenderHeaps}.
 */
export async function installRenderHeapScanner(page: Page): Promise<void> {
  await page.addInitScript((prologue: string) => {
    const w = window as unknown as {
      __scanWorkers: Worker[];
      __scanWrapped: Set<string>;
      __scanReplies: Map<number, (result: unknown) => void>;
    };
    w.__scanWorkers = [];
    w.__scanWrapped = new Set();
    w.__scanReplies = new Map();
    const create = URL.createObjectURL.bind(URL);
    URL.createObjectURL = (object: Blob | MediaSource) => {
      if (object instanceof Blob && object.type === "text/javascript") {
        const url = create(new Blob([prologue, "\n", object], { type: "text/javascript" }));
        w.__scanWrapped.add(url);
        return url;
      }
      return create(object);
    };
    const NativeWorker = window.Worker;
    window.Worker = class extends NativeWorker {
      constructor(url: string | URL, options?: WorkerOptions) {
        super(url, options);
        // Only a worker built from a blob that got the prologue can answer a scan; which of
        // those is the render worker is decided by the memories it reports, not by spawn order.
        if (w.__scanWrapped.has(String(url))) w.__scanWorkers.push(this);
        // Registered first, so the host's listeners never see a scan answer.
        this.addEventListener("message", (event: MessageEvent) => {
          const nonce = event.data && event.data.__burrowScanned;
          if (typeof nonce !== "number") return;
          event.stopImmediatePropagation();
          w.__scanReplies.get(nonce)?.(event.data.result);
          w.__scanReplies.delete(nonce);
        });
      }
    } as typeof Worker;
  }, PROLOGUE);
}

/**
 * Scan every memory of the render worker for `needles`. The worker is identified by what it
 * reports, not by when it was spawned: every worker that carries the prologue is asked, and the
 * one whose memories include both a `pdfium` and a `burrow-render` heap is the render worker.
 * Zero or more than one such worker is an error rather than a guess.
 */
export async function scanRenderHeaps(page: Page, needles: Needle[]): Promise<MemoryScan[]> {
  return page.evaluate(
    (needles) =>
      new Promise<MemoryScan[]>((resolve, reject) => {
        const w = window as unknown as {
          __scanWorkers: Worker[];
          __scanReplies: Map<number, (result: unknown) => void>;
        };
        const workers = [...w.__scanWorkers];
        if (workers.length === 0) {
          reject(new Error("no worker was spawned, so there is nothing to scan"));
          return;
        }
        const answers: MemoryScan[][] = [];
        let pending = workers.length;
        const done = () => {
          if (--pending > 0) return;
          const render = answers.filter(
            (scan) =>
              scan.some((m) => m.heap === "pdfium") && scan.some((m) => m.heap === "burrow-render"),
          );
          if (render.length === 1) resolve(render[0]);
          else
            reject(
              new Error(
                `${render.length} of ${workers.length} workers hold pdfium and burrow-render memories`,
              ),
            );
        };
        for (const worker of workers) {
          const nonce = Math.floor(Math.random() * 2 ** 31);
          // A worker the host has already terminated never answers: skip it after a pause.
          const timer = setTimeout(() => {
            w.__scanReplies.delete(nonce);
            done();
          }, 10_000);
          w.__scanReplies.set(nonce, (result) => {
            clearTimeout(timer);
            answers.push(result as MemoryScan[]);
            done();
          });
          worker.postMessage({ __burrowScan: { nonce, needles } });
        }
      }),
    needles,
  );
}
