#!/usr/bin/env node
// Raw / gzip / brotli sizes for the artifacts an option actually ships.
// Node's zlib provides brotli, so there is no extra dependency to install.
import { readFileSync, statSync } from "node:fs";
import { gzipSync, brotliCompressSync, constants } from "node:zlib";
import path from "node:path";

const files = process.argv.slice(2);
const pad = (s, n) => String(s).padEnd(n);
const num = (n) => n.toLocaleString("en-US").padStart(12);

console.log(pad("artifact", 30) + num("raw") + num("gzip") + num("brotli"));
let tr = 0, tg = 0, tb = 0;
for (const f of files) {
  let buf;
  try { buf = readFileSync(f); } catch { continue; }
  const r = statSync(f).size;
  const g = gzipSync(buf, { level: 9 }).length;
  const b = brotliCompressSync(buf, {
    params: { [constants.BROTLI_PARAM_QUALITY]: 11 },
  }).length;
  console.log(pad(path.basename(f), 30) + num(r) + num(g) + num(b));
  tr += r; tg += g; tb += b;
}
console.log(pad("TOTAL", 30) + num(tr) + num(tg) + num(tb));
