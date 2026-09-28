// What R8 and R9 say about redaction's worker, as functions of what passed between it and the
// page (#137).
//
// ADR 0006's R8 and R9 are conditions of an accepted decision, and "none of the three is satisfied
// until its named check exists and has been shown to fail without the property". The two specs
// that use this file are those checks. The predicates live here, apart from either spec, so the
// function a real redaction passes is the SAME function a planted violation must fail -- a
// predicate rewritten for the mutation case would prove only that it can be written to fail.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import type { Page } from "@playwright/test";

import type { RedactionMessage, Reply } from "../src/host/harness-api";
import type { LogEntry } from "./request-log";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");

/** A committed fixture a whole-page redaction succeeds on, pinned in `outcomes.tsv`. */
export const WRITER = readFileSync(join(repo, "tests/redaction/fixtures/producer-writer.pdf"));

export const WHOLE = { left: 0, top: 0, width: 5000, height: 5000 };

/** THE LINE A PLANTED VIOLATION GOES BEFORE: `redact-main.js`'s one call into Rust. */
export const THE_CALL = "  return wasm_bindgen.redact(";

/** The settle: the worker echoes after this long, and the page reads the log `2 *` this after the send. */
export const SETTLE_MS = 500;

/** What `redactAndSettle` hands back: the reply, the log, and the nonce the log must echo. */
export interface Settled {
  reply: Reply;
  log: RedactionMessage[];
  nonce: number;
}

/**
 * Redact `WRITER`'s `page` (1-based) through redaction's worker, as armed, then wait for the
 * worker to go quiet and read the log. "Posted once" read at the moment of the reply is "posted
 * once so far": a copy that posted the document a second time 200 ms later passed that version of
 * this check (review of #137).
 */
export async function redactAndSettle(page: Page, pageNumber: number): Promise<Settled> {
  const reply = await page.evaluate(
    ({ bytes, pageNumber, region }) =>
      window.burrowHarness.redactDocument(
        "producer-writer.pdf",
        new Uint8Array(bytes),
        pageNumber,
        [pageNumber],
        region,
      ),
    { bytes: Array.from(WRITER), pageNumber, region: WHOLE },
  );
  const { settled, nonce } = await page.evaluate(
    (ms) => window.burrowHarness.settleRedaction(ms),
    SETTLE_MS,
  );
  if (!settled) throw new Error("redaction's worker never answered the settle handshake");
  const log = await page.evaluate(() => window.burrowHarness.redactMessages());
  return { reply, log, nonce };
}

/** The id of the one redaction the page sent in `log`. Refuses if it did not send exactly one. */
export function onlyRequest(log: RedactionMessage[]): number {
  const ids = log.filter((m) => m.sent === "redact").map((m) => m.id);
  if (ids.length !== 1 || typeof ids[0] !== "number") {
    throw new Error(`expected one redaction request, found ${JSON.stringify(ids)}`);
  }
  return ids[0];
}

/** A plain object's shape, as `shapeOf` in `harness-driver.js` writes one. */
function shape(fields: Record<string, string>): string {
  return `{${Object.entries(fields)
    .map(([key, type]) => `${key}:${type}`)
    .sort()
    .join(",")}}`;
}

const LIMITS = shape({
  maxDurationMs: "number",
  maxInputBytes: "number",
  maxMemoryBytes: "number",
  maxPages: "number",
  maxPixels: "number",
});

/**
 * EVERY SHAPE THE WORKER MAY POST BEFORE ITS REPLY, exactly, and in which group: `prelude.js`'
 * `starting` once per engine module (two in this bundle), ONE answer to `init` -- ready or not --
 * and the ack. Measured on the real worker in all three browsers, not read off the code. Anything
 * else, one of these too often, or one of these AFTER the reply, is a violation.
 */
const EARLY: Record<string, string> = {
  [shape({ starting: "true" })]: "starting",
  [shape({
    defaultLimits: LIMITS,
    id: "number",
    minConvergingMemoryBytes: "string",
    ready: "true",
  })]: "init",
  [shape({ fatal: "true", id: "number", kind: "string", ready: "false" })]: "init",
  [shape({ ack: "true", id: "number" })]: "ack",
};
const EARLY_COUNT: Record<string, number> = { starting: 2, init: 1, ack: 1 };

/**
 * EVERY SHAPE A REPLY MAY TAKE, exactly: `drainReply` on success and on refusal, and the three
 * literals `worker-protocol.js` and `redact-main.js` build by hand -- `refusal`, the unknown-op
 * refusal and `internalFailure`. The success shape is the only one with bytes in it, and only in
 * `output`: that is ADR 0023's "exactly one part", held by the shape rather than by a count.
 * `rotations` is empty in every one, because redaction never fills it.
 */
const REPLIES: Set<string> = (() => {
  const shapes = new Set<string>();
  const drained = (ok: string, output: string, fatal: string, recycle: string) =>
    shape({
      allowed: "string",
      droppedCarriedText: "number",
      engineHeapBytes: "string",
      failedInput: "number",
      fatal,
      id: "number",
      innerKind: "string",
      kind: "string",
      limit: "string",
      message: "string",
      ok,
      originalBytes: "string",
      output,
      pages: "number",
      producedBytes: "string",
      recycle,
      report: "string",
      requested: "string",
      retainedFonts: "number",
      rotations: "[]",
      stage: "string",
    });
  for (const fatal of ["true", "false"]) {
    for (const recycle of ["true", "false"]) {
      shapes.add(drained("true", "bytes", fatal, recycle));
      shapes.add(drained("false", "null", fatal, recycle));
    }
  }
  const common = {
    allowed: "string",
    engineHeapBytes: "string",
    fatal: "false",
    id: "number",
    kind: "string",
    limit: "string",
    message: "string",
    ok: "false",
    pages: "number",
    recycle: "false",
    requested: "string",
    stage: "string",
  };
  shapes.add(shape({ ...common, failedInput: "number", innerKind: "string", rotations: "[]" }));
  shapes.add(shape(common));
  shapes.add(shape({ ...common, allowed: "number", fatal: "true", requested: "number" }));
  return shapes;
})();

/** `kind_of` in `bindings/burrow-wasm/src/lib.rs`, every arm. A new one fails this spec, loudly. */
const KINDS = [
  "Malformed",
  "Unsupported",
  "PasswordRequired",
  "LimitExceeded",
  "InvalidArgument",
  "InputFailed",
  "Io",
  "OutputRejected",
  "Internal",
  "Unknown",
];
/** `Stage::as_str` in `core/burrow-types/src/stage.rs`, every arm, and "" for none. */
const STAGES = [
  "",
  "input_size",
  "size_estimate",
  "prescan",
  "page_count",
  "pixels",
  "measured",
  "deadline",
];
/** The `Limits` fields a `LimitExceeded` can name, and "" for none. */
const LIMIT_NAMES = [
  "",
  "max_input_bytes",
  "max_memory_bytes",
  "max_duration_ms",
  "max_pages",
  "max_pixels",
];

const U32 = 2 ** 32;
const U64_MAX = 2n ** 64n - 1n;
/** A whole number in range, and not `-0`: a sign bit is a bit. */
const integer = (low: number, high: number) => (v: unknown) =>
  typeof v === "number" && Number.isInteger(v) && !Object.is(v, -0) && v >= low && v <= high;
/** A `u64` as the worker writes one: canonical digits -- no leading zero -- no larger than 2^64 - 1. */
const u64 = (v: unknown) =>
  (typeof v === "string" && /^(0|[1-9]\d{0,19})$/.test(v) && BigInt(v) <= U64_MAX) ||
  integer(0, Number.MAX_SAFE_INTEGER)(v);
const oneOf = (values: string[]) => (v: unknown) => typeof v === "string" && values.includes(v);

/** A canonical whole number, as digits: no leading zero, no sign. */
const DIGITS = "(0|[1-9]\\d*)";
const FONT = `FontOutcome \\{ font: ${DIGITS}, cut: (true|false), also_used_by: ${DIGITS} \\}`;
const REPORT_GRAMMAR = new RegExp(
  `^Report \\{ fonts: \\[(${FONT}(, ${FONT})*)?\\], dropped_carried_text: ${DIGITS} \\}$`,
);
/** The report's ceiling on fonts: the regex's, not the report's -- a page with more fails closed. */
const FONTS_MAX = 64;
const U32_MAX = 2n ** 32n - 1n;
/** `(number << 16) | generation`, a 32-bit object number and a 16-bit generation. */
const FONT_ID_MAX = 2n ** 48n - 1n;

/**
 * `format!("{report:?}")` of `burrow_ops::redact::Report`, as `Reply::redacted` writes it on a
 * success: fonts, each an object identity, a flag and a count, then one more count -- integers and
 * booleans, nothing else, each canonical and in its type's range. NOT PROSE, and it was called
 * that until review found the whole document passing in it (#137). A `String` added to `Report`
 * would now fail this spec rather than widen it silently.
 */
function isReport(v: unknown): boolean {
  if (typeof v !== "string" || !REPORT_GRAMMAR.test(v)) return false;
  const fonts = [...v.matchAll(/font: (\d+), cut: (?:true|false), also_used_by: (\d+)/g)];
  const dropped = /dropped_carried_text: (\d+) \}$/.exec(v);
  return (
    fonts.length <= FONTS_MAX &&
    fonts.every(([, font, users]) => BigInt(font) <= FONT_ID_MAX && BigInt(users) <= U32_MAX) &&
    dropped !== null &&
    BigInt(dropped[1]) <= U32_MAX
  );
}

/**
 * WHAT EACH FIELD'S VALUE MAY BE, where the shape says only its type. Enumerations are copied from
 * the Rust that produces them; numbers are whole, bounded and never `-0`; the two sizes that only
 * `compress` fills are pinned to "0"; a success's `message` is empty and its output a PDF.
 *
 * WHAT STILL FITS IS NUMBER-SHAPED, and this is its arithmetic rather than an estimate:
 * - three `u64` fields at 8 bytes, five 32-bit counts at 4, the enumerations' and booleans' few
 *   bits, and about 8 bytes in the order of the reply's keys (the shape sorts them): some sixty
 *   bytes a reply;
 * - and a success's report: per font, a 48-bit identity, a 32-bit count and a flag, about 10 bytes,
 *   up to 64 fonts -- about 650 bytes more at that ceiling.
 * Not text, and not a name a person would recognise as the secret; stated, not closed. (The first
 * version of this said "about 4 bytes a font", counting one of the font's two numbers.)
 */
export const FIELD_RULES: Record<string, (v: unknown) => boolean> = {
  id: integer(0, U32),
  // Empty on a success, measured: `kind` names an error.
  kind: oneOf(["", ...KINDS]),
  innerKind: oneOf(["", ...KINDS]),
  stage: oneOf(STAGES),
  limit: oneOf(LIMIT_NAMES),
  allowed: u64,
  requested: u64,
  engineHeapBytes: u64,
  // Only `compress` fills these; every redaction writes "0", measured.
  originalBytes: oneOf(["0"]),
  producedBytes: oneOf(["0"]),
  minConvergingMemoryBytes: u64,
  pages: integer(0, U32),
  retainedFonts: integer(0, U32),
  droppedCarriedText: integer(0, U32),
  failedInput: integer(-1, U32),
  message: oneOf([""]),
  report: isReport,
  // A refusal's report, recorded apart so it can be held empty.
  refusalReport: oneOf([""]),
  "output.type": oneOf(["application/pdf"]),
  "defaultLimits.maxDurationMs": integer(0, Number.MAX_SAFE_INTEGER),
  "defaultLimits.maxInputBytes": integer(0, Number.MAX_SAFE_INTEGER),
  "defaultLimits.maxMemoryBytes": integer(0, Number.MAX_SAFE_INTEGER),
  "defaultLimits.maxPages": integer(0, Number.MAX_SAFE_INTEGER),
  "defaultLimits.maxPixels": integer(0, Number.MAX_SAFE_INTEGER),
  __burrowSettled: integer(1, 2 ** 31),
  __burrowSideChannel: (v) => typeof v === "string" && Object.hasOwn(EXITS, v),
};

/** The fields whose values are numbers; every other rule is over a string. */
const NUMERIC = new Set([
  "id",
  "pages",
  "retainedFonts",
  "droppedCarriedText",
  "failedInput",
  "defaultLimits.maxDurationMs",
  "defaultLimits.maxInputBytes",
  "defaultLimits.maxMemoryBytes",
  "defaultLimits.maxPages",
  "defaultLimits.maxPixels",
  "__burrowSettled",
]);

/**
 * VALUES EACH TIGHTENING REFUSES, which a blunt value cannot show: `REFUSED_BY` proves each rule is
 * there, and every loosening of these -- a leading zero, 2^64, `-0`, a pinned size, a 65th font, a
 * font identity past 48 bits -- stayed green against it (review of #137).
 */
const FONT_OK = "FontOutcome { font: 851968, cut: true, also_used_by: 0 }";
const report = (fonts: string[], dropped = "0") =>
  `Report { fonts: [${fonts.join(", ")}], dropped_carried_text: ${dropped} }`;
export const REFINED: [string, string | number, string][] = [
  ["allowed", "01", "a leading zero"],
  ["allowed", "18446744073709551616", "2^64"],
  ["pages", -0, "-0"],
  ["originalBytes", "1", "a size only compress fills"],
  ["producedBytes", "1", "a size only compress fills"],
  ["report", report(Array.from({ length: FONTS_MAX + 1 }, () => FONT_OK)), "a 65th font"],
  ["report", report([FONT_OK.replace("851968", "281474976710656")]), "a font identity of 2^48"],
  [
    "report",
    report([FONT_OK.replace("also_used_by: 0", "also_used_by: 4294967296")]),
    "a count of 2^32",
  ],
  ["report", report([FONT_OK.replace("851968", "0851968")]), "a font identity with a leading zero"],
  ["report", report([FONT_OK], "4294967296"), "a dropped count of 2^32"],
];

/** A value each rule refuses, for the hand-written case that shows the rule is there. */
export const REFUSED_BY: Record<string, string | number> = Object.fromEntries(
  Object.keys(FIELD_RULES).map((key) => [key, NUMERIC.has(key) ? 1.5 : "x y"]),
);

/** Every field value in `m` no rule admits, named. Booleans and `null` are the shape's to fix. */
function badFields(m: RedactionMessage): string[] {
  return Object.entries(m.fields)
    .filter(([, value]) => typeof value === "string" || typeof value === "number")
    .filter(([key, value]) => {
      const rule = FIELD_RULES[key];
      return rule === undefined || !rule(value);
    })
    .map(([key]) => key);
}

/**
 * The harness's own messages, EXACTLY:
 * - the armed list, ONCE, as the worker's very first message -- the prologue always posts one, empty
 *   when nothing is armed -- naming only exits a stub exists for, each once;
 * - a stub's report, naming an exit that armed list named;
 * - the settle echo carrying THIS nonce.
 * Each earlier version exempted more: anything beginning `__burrow`, then an armed list at any
 * count carrying a document three bits a name, with `in` walking the prototype chain (reviews of
 * #137).
 */
function harnessOwn(m: RedactionMessage, at: number, nonce: number, armed: string[]): boolean {
  const exit = (name: unknown) => typeof name === "string" && Object.hasOwn(EXITS, name);
  if (m.shape === shape({ __burrowSettled: "number" })) return m.settled === nonce;
  // ONLY FOR AN EXIT THE ARMED LIST NAMED: with no stubs armed there are no reports, and 1800
  // forged ones carried 600 bytes past a version that accepted any (review of #137).
  if (m.shape === shape({ __burrowSideChannel: "string" })) {
    return exit(m.sideChannel) && armed.includes(m.sideChannel ?? "");
  }
  if (
    m.shape === shape({ __burrowSideChannelArmed: "[string]" }) ||
    m.shape === shape({ __burrowSideChannelArmed: "[]" })
  ) {
    const names = m.armed ?? [];
    return at === 0 && names.every(exit) && new Set(names).size === names.length;
  }
  return false;
}

/**
 * R8: the output is ONE value, posted after the Rust call returned. Returns every way `all`
 * breaks it, or nothing. DENY BY DEFAULT, over EVERY message the worker posted whatever id it
 * names, because each version of this that listed what to look for was got past by something it
 * did not list (reviews of #137):
 *
 * - The log contains THIS nonce's settle echo, or "nothing after the reply" was never observed.
 * - The terminal reply is the last message for the request that has an `ok`, and its shape is one
 *   of `REPLIES`.
 * - Every other message is the harness's own (exactly), or one of `EARLY`, BEFORE the reply and no
 *   more often than its group allows. A message of any other shape is refused -- as "a second
 *   reply" when it has an `ok`.
 * - No message transfers a port.
 * - Every field's value is one `FIELD_RULES` admits.
 *
 * WHAT A REFUSAL'S `message` SAYS IS NOT CHECKED, at any length: it is Rust's prose, and an error
 * quoting the document would pass. The typed-error rule holds that, not this spec. `report` is not
 * prose -- it is held to its `Debug` grammar -- and a success's `message` must be empty.
 */
export function r8Violations(all: RedactionMessage[], id: number, nonce: number): string[] {
  const log = all.filter((m) => m.sent === null);
  const found: string[] = [];
  if (!log.some((m) => m.settled === nonce)) {
    found.push("the log was read before the worker went quiet");
  }
  const replies = log.filter((m) => m.id === id && m.ok !== null);
  const terminal = replies.length > 0 ? replies[replies.length - 1] : null;
  if (terminal === null) found.push("no terminal reply");
  const after = terminal === null ? log.length : log.indexOf(terminal);
  const first = log[0];
  const armedList =
    first !== undefined && first.armed !== null && harnessOwn(first, 0, nonce, [])
      ? first.armed
      : [];
  const seen = new Map<string, number>();
  log.forEach((m, at) => {
    const what = `message ${at} (id ${m.id}, keys ${m.keys})`;
    if (m.ports > 0) found.push(`${what} transfers ${m.ports} port(s)`);
    const bad = badFields(m);
    if (bad.length > 0) found.push(`${what} has a value no rule admits in ${bad}`);
    if (m === terminal) {
      if (!REPLIES.has(m.shape)) {
        found.push(`the terminal reply has a shape nobody listed: ${m.shape.slice(0, 200)}`);
      }
      return;
    }
    if (harnessOwn(m, at, nonce, armedList)) return;
    const group = EARLY[m.shape];
    if (group === undefined) {
      found.push(
        m.ok !== null
          ? `${what} is a second reply`
          : `${what} has a shape nobody listed: ${m.shape.slice(0, 200)}`,
      );
      return;
    }
    if (at > after) found.push(`${what} is a ${group} message after the reply`);
    const count = (seen.get(group) ?? 0) + 1;
    seen.set(group, count);
    if (count === EARLY_COUNT[group] + 1) found.push(`${what} is one ${group} too many`);
  });
  return found;
}

/**
 * R9, HELD MORE STRICTLY THAN ADR 0006 WORDS IT. The ADR says no exit before verification; this
 * says NO EXIT AT ALL, from arming until the settle. Redaction's worker has no reason to take one
 * at any point -- the page makes the download link from the Blob it is handed -- and the stricter
 * rule is what catches an exit on a REFUSAL, where nothing was ever verified, and one a timer
 * deferred past the reply (review of #137). Returns every exit taken, or nothing.
 *
 * An exit later than `2 * SETTLE_MS` after the settle handshake is not seen; that bound is stated,
 * not closed. And the stubs are a TRIPWIRE FOR A REGRESSION, not a boundary against code that sets
 * out to evade them: they share the worker's realm, and a bundle determined to take an exit unseen
 * is not what this spec is for.
 */
export function r9Violations(all: RedactionMessage[], nonce: number): string[] {
  const log = all.filter((m) => m.sent === null);
  const found = log
    .map((m, at) => ({ m, at }))
    .filter(({ m }) => m.sideChannel !== null)
    .map(({ m, at }) => `${m.sideChannel} was called at message ${at}`);
  if (!log.some((m) => m.settled === nonce)) {
    found.push("the log was read before the worker went quiet");
  }
  return found;
}

/**
 * The request paths a redaction may make: its bundle, its two modules, and the policy guard's
 * control -- the request `prelude.js` pairs with a probe the CSP must refuse, which every bundle
 * makes before it touches a file (measured in all three browsers; its probe never reaches the
 * server, because the browser refuses it). A PATTERN over the content hash, not the build's exact
 * URLs: the CSP pins those, and a request to another hash would need `connect-src` to regress
 * first -- a gap this check does not close, stated.
 */
const REDACTION_ARTIFACT =
  /^\/engines\/(burrow-redact-worker\.[0-9a-f]{16}\.js|(qpdf|burrow_wasm_redact_bg)\.[0-9a-f]{16}\.wasm|control\.[0-9a-f]{16}\.txt)$/;

/**
 * The network half of R9, from the test server's own log: between a marker written after arming
 * and the settle, every request is a GET from `site` to one of `REDACTION_ARTIFACT`'s four paths,
 * each at most once. The query string is the point -- `connect-src` admits
 * `…/qpdf.<hash>.wasm?leak=…`, and a copy of this worker sent the input's first bytes that way
 * while every stub stayed silent (review of #137). `e2e/zero-requests.spec.ts` documents the hole;
 * this is the check for the worker holding the most sensitive content.
 */
export function networkViolations(entries: LogEntry[], site: string): string[] {
  const found: string[] = [];
  const seen = new Set<string>();
  for (const entry of entries) {
    const what = `${entry.method} ${entry.origin}${entry.url.slice(0, 120)}`;
    if (entry.origin !== site || entry.method !== "GET" || !REDACTION_ARTIFACT.test(entry.url)) {
      found.push(`${what} is not a GET of one of redaction's artifacts`);
    } else if (seen.has(entry.url)) {
      found.push(`${entry.url} was requested twice`);
    }
    seen.add(entry.url);
  }
  return found;
}

/** The stubs the prologue reports it installed, from its first message. */
export function armedStubs(log: RedactionMessage[]): string[] {
  return log.find((m) => m.armed !== null)?.armed ?? [];
}

/**
 * Every exit a stub exists for, and how to ask a worker whether it can take it. Asked of a
 * throwaway worker rather than the page, because the two scopes differ and the exit is the
 * worker's. The names are the stubs' own, so "armed" and "reachable" compare as sets.
 */
export const EXITS: Record<string, string> = {
  createObjectURL: "typeof URL.createObjectURL === 'function'",
  showSaveFilePicker: "typeof self.showSaveFilePicker === 'function'",
  getDirectory: "typeof StorageManager !== 'undefined'",
  BroadcastChannel: "typeof BroadcastChannel !== 'undefined'",
  indexedDB: "typeof IDBFactory !== 'undefined'",
  caches: "typeof CacheStorage !== 'undefined'",
  locks: "typeof LockManager !== 'undefined'",
  Worker: "typeof Worker === 'function'",
  SharedWorker: "typeof SharedWorker === 'function'",
};

/**
 * A planted exit for each, taken before the call: what a copy of the worker does to be caught.
 * `bytes` is the input, in scope at `THE_CALL`. `showSaveFilePicker` has none, because no worker
 * scope measured has one to call.
 */
export const PLANTS: Record<string, string> = {
  createObjectURL: "URL.createObjectURL(new Blob([bytes]));",
  getDirectory: "navigator.storage.getDirectory();",
  BroadcastChannel: 'new BroadcastChannel("out").postMessage(bytes);',
  indexedDB: 'indexedDB.open("out");',
  caches: 'caches.open("out");',
  locks: 'navigator.locks.request("out", () => {});',
  Worker: 'new Worker(self.location.href, { name: "nested" });',
  SharedWorker: 'new SharedWorker(self.location.href, { name: "shared" });',
};

/** Which of `EXITS` a worker in this browser can reach. */
export async function reachableExits(page: Page): Promise<string[]> {
  const source = `self.postMessage({ ${Object.entries(EXITS)
    .map(([name, test]) => `${name}: ${test}`)
    .join(", ")} })`;
  const reachable = await page.evaluate(async (source) => {
    const probe = new Worker(URL.createObjectURL(new Blob([source], { type: "text/javascript" })));
    return new Promise<Record<string, boolean>>((done) => {
      probe.onmessage = (event) => {
        done(event.data);
        probe.terminate();
      };
    });
  }, source);
  return Object.keys(EXITS).filter((name) => reachable[name] === true);
}

/**
 * A log written by hand that every rule passes: the request, a successful reply of the listed
 * shape, and the settle echo. Each hand-written case changes ONE thing in it, so the finding it
 * provokes is that rule's and nothing else's -- and this, unchanged, is the near-miss beside all
 * of them.
 */
export function cleanLog(id: number, nonce: number): RedactionMessage[] {
  const blank = {
    keys: [],
    bytes: 0,
    fields: {},
    ports: 0,
    ok: null,
    sideChannel: null,
    armed: null,
    settled: null,
  };
  const success = [...REPLIES].find(
    (s) => s.includes("ok:true") && s.includes("fatal:false") && s.includes("recycle:false"),
  );
  if (success === undefined) throw new Error("no successful reply shape is listed");
  return [
    { ...blank, sent: "redact", id, keys: ["blob", "id", "op"], bytes: 1, shape: "request" },
    {
      ...blank,
      sent: null,
      id,
      keys: ["id", "ok", "output"],
      bytes: 1,
      ok: true,
      shape: success,
      // A SUCCESS AS THE REAL WORKER SENDS ONE, field for field -- `report` as measured on
      // `producer-writer.pdf` -- so a case that changes one field provokes that field's rule alone.
      fields: {
        id,
        ok: true,
        fatal: false,
        recycle: false,
        kind: "",
        innerKind: "",
        stage: "",
        limit: "",
        message: "",
        allowed: "0",
        requested: "0",
        engineHeapBytes: "20971520",
        originalBytes: "0",
        producedBytes: "0",
        pages: 0,
        retainedFonts: 0,
        droppedCarriedText: 0,
        failedInput: -1,
        report:
          "Report { fonts: [FontOutcome { font: 1179648, cut: true, also_used_by: 0 }, FontOutcome { font: 851968, cut: true, also_used_by: 0 }], dropped_carried_text: 0 }",
        "output.type": "application/pdf",
      },
    },
    {
      ...blank,
      sent: null,
      id: null,
      keys: ["__burrowSettled"],
      settled: nonce,
      shape: shape({ __burrowSettled: "number" }),
      fields: { __burrowSettled: nonce },
    },
  ];
}
