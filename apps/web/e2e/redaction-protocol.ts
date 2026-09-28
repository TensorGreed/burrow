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

/** How long the worker is given to go quiet after its reply: on its clock, then on the page's. */
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
const integer = (low: number, high: number) => (v: unknown) =>
  typeof v === "number" && Number.isInteger(v) && v >= low && v <= high;
const u64 = (v: unknown) =>
  (typeof v === "string" && /^\d{1,20}$/.test(v)) || integer(0, Number.MAX_SAFE_INTEGER)(v);
const oneOf = (values: string[]) => (v: unknown) => typeof v === "string" && values.includes(v);

/**
 * WHAT EACH FIELD'S VALUE MAY BE, where the shape says only its type. Enumerations are copied from
 * the Rust that produces them; numbers are whole and bounded, so no NaN payload and no 53-bit
 * double rides in one. What still fits is NUMBER-SHAPED: about 4 bytes per 32-bit field and 8 per
 * `u64`, some ninety bytes a reply -- not text, and not a name or a number a person would recognise
 * as the secret, but not nothing, and stated rather than closed.
 */
const FIELD_RULES: Record<string, (v: unknown) => boolean> = {
  id: integer(0, U32),
  // Empty on a success, measured: `kind` names an error.
  kind: oneOf(["", ...KINDS]),
  innerKind: oneOf(["", ...KINDS]),
  stage: oneOf(STAGES),
  limit: oneOf(LIMIT_NAMES),
  allowed: u64,
  requested: u64,
  engineHeapBytes: u64,
  originalBytes: u64,
  producedBytes: u64,
  minConvergingMemoryBytes: u64,
  pages: integer(0, U32),
  retainedFonts: integer(0, U32),
  droppedCarriedText: integer(0, U32),
  failedInput: integer(-1, U32),
  "defaultLimits.maxDurationMs": integer(0, Number.MAX_SAFE_INTEGER),
  "defaultLimits.maxInputBytes": integer(0, Number.MAX_SAFE_INTEGER),
  "defaultLimits.maxMemoryBytes": integer(0, Number.MAX_SAFE_INTEGER),
  "defaultLimits.maxPages": integer(0, Number.MAX_SAFE_INTEGER),
  "defaultLimits.maxPixels": integer(0, Number.MAX_SAFE_INTEGER),
  __burrowSettled: integer(1, 2 ** 31),
  __burrowSideChannel: (v) => typeof v === "string" && Object.hasOwn(EXITS, v),
};

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
 * - the armed list, ONCE, as the worker's very first message, naming only exits a stub exists for;
 * - a stub's report, naming such an exit;
 * - the settle echo carrying THIS nonce.
 * Each earlier version exempted more: anything beginning `__burrow`, then an armed list at any
 * count carrying a document three bits a name, with `in` walking the prototype chain (reviews of
 * #137).
 */
function harnessOwn(m: RedactionMessage, at: number, nonce: number): boolean {
  const exit = (name: unknown) => typeof name === "string" && Object.hasOwn(EXITS, name);
  if (m.shape === shape({ __burrowSettled: "number" })) return m.settled === nonce;
  if (m.shape === shape({ __burrowSideChannel: "string" })) return exit(m.sideChannel);
  if (
    m.shape === shape({ __burrowSideChannelArmed: "[string]" }) ||
    m.shape === shape({ __burrowSideChannelArmed: "[]" })
  ) {
    return at === 0 && (m.armed ?? []).every(exit);
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
 * WHAT `message` AND `report` SAY IS NOT CHECKED: they are Rust's prose, and an error quoting the
 * document would pass. The typed-error rule holds that, not this spec.
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
    if (harnessOwn(m, at, nonce)) return;
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
      fields: { id, ok: true, kind: "", stage: "", pages: 0 },
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
