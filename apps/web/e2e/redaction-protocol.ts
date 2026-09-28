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

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");

/** A committed fixture a whole-page redaction succeeds on, pinned in `outcomes.tsv`. */
export const WRITER = readFileSync(join(repo, "tests/redaction/fixtures/producer-writer.pdf"));

export const WHOLE = { left: 0, top: 0, width: 5000, height: 5000 };

/** THE LINE A PLANTED VIOLATION GOES BEFORE: `redact-main.js`'s one call into Rust. */
export const THE_CALL = "  return wasm_bindgen.redact(";

/** How long the worker is given to go quiet after its reply, on its own clock. */
export const SETTLE_MS = 500;

/**
 * Redact `WRITER`'s `page` (1-based) through redaction's worker, as armed, then wait for the
 * worker to go quiet and read the log. "Posted once" read at the moment of the reply is "posted
 * once so far": a copy that posted the document a second time 200 ms later passed that version of
 * this check (review of #137).
 */
export async function redactAndSettle(
  page: Page,
  pageNumber: number,
): Promise<{ reply: Reply; log: RedactionMessage[] }> {
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
  const { settled } = await page.evaluate(
    (ms) => window.burrowHarness.settleRedaction(ms),
    SETTLE_MS,
  );
  if (!settled) throw new Error("redaction's worker never answered the settle handshake");
  const log = await page.evaluate(() => window.burrowHarness.redactMessages());
  return { reply, log };
}

/** The id of the one redaction the page sent in `log`. Refuses if it did not send exactly one. */
export function onlyRequest(log: RedactionMessage[]): number {
  const ids = log.filter((m) => m.sent === "redact").map((m) => m.id);
  if (ids.length !== 1 || typeof ids[0] !== "number") {
    throw new Error(`expected one redaction request, found ${JSON.stringify(ids)}`);
  }
  return ids[0];
}

/** Whether a worker message is the harness's own -- a stub's report, the armed list, the marker. */
function harnessOwn(m: RedactionMessage): boolean {
  return m.keys.length === 1 && m.keys[0].startsWith("__burrow");
}

/**
 * EVERY SHAPE THE WORKER MAY POST BEFORE ITS REPLY, as exact key sets: `prelude.js`' `starting`,
 * the two answers to `init`, and the ack. Anything else before the reply is content in a shape
 * nobody listed, which is where a number array or a string of the document would be.
 */
const EARLY_SHAPES = [
  ["starting"],
  ["defaultLimits", "id", "minConvergingMemoryBytes", "ready"],
  ["fatal", "id", "kind", "ready"],
  ["ack", "id"],
].map((keys) => keys.join(","));

/**
 * The longest string or array an early message may hold. The largest legitimate one is a limit
 * rendered as a decimal string (20 digits for a `u64`); a document is not going to fit in 64.
 */
const EARLY_LONGEST = 64;

/**
 * Every field a reply may carry: the union of `drainReply`, `refusal`, the unknown-op refusal and
 * `internalFailure` in `worker-protocol.js`. Its strings `message` and `report` are Rust's, and
 * WHAT THEY SAY IS NOT CHECKED HERE: an error message quoting the document would pass. That is
 * the typed-error rule's to hold, not this spec's.
 */
const REPLY_KEYS = new Set([
  "allowed",
  "droppedCarriedText",
  "engineHeapBytes",
  "failedInput",
  "fatal",
  "id",
  "innerKind",
  "kind",
  "limit",
  "message",
  "ok",
  "originalBytes",
  "output",
  "pages",
  "producedBytes",
  "recycle",
  "report",
  "requested",
  "retainedFonts",
  "rotations",
  "stage",
]);

/**
 * R8: the output is ONE value, posted after the Rust call returned. Returns every way `all`
 * breaks it, or nothing. Over EVERY message the worker posted, whatever id it names -- a message
 * under a foreign id or none still reached the page's thread, and an earlier version that looked
 * only at null and the request's own id passed three planted chunks (review of #137):
 *
 * - The terminal reply is the last message for the request that has an `ok`. A successful one
 *   carries exactly ONE byte-carrying value -- ADR 0023's "one part, not merely one message" --
 *   and a refusal carries none.
 * - Every other message carries no bytes and transfers no port, has a shape `EARLY_SHAPES` lists,
 *   and holds nothing longer than `EARLY_LONGEST`. That includes anything posted AFTER the reply,
 *   up to the settle marker.
 * - The terminal reply has no field outside `REPLY_KEYS` and transfers no port.
 * - The log must end in the settle marker, or "nothing after the reply" was never observed.
 */
export function r8Violations(all: RedactionMessage[], id: number): string[] {
  const log = all.filter((m) => m.sent === null);
  const found: string[] = [];
  if (!log.some((m) => m.keys.includes("__burrowSettled"))) {
    found.push("the log was read before the worker went quiet");
  }
  const replies = log.filter((m) => m.id === id && m.ok !== null);
  const terminal = replies.length > 0 ? replies[replies.length - 1] : null;
  if (terminal === null) found.push("no terminal reply");
  log.forEach((m, at) => {
    if (m === terminal || harnessOwn(m)) return;
    const what = `message ${at} (id ${m.id}, keys ${m.keys})`;
    if (m.bytes > 0) found.push(`${what} carries bytes, and is not the terminal reply`);
    if (m.ports > 0) found.push(`${what} transfers ${m.ports} port(s)`);
    if (m.ok !== null) found.push(`${what} is a second reply`);
    else if (!EARLY_SHAPES.includes(m.keys.join(",")))
      found.push(`${what} has a shape nobody listed`);
    if (m.longest > EARLY_LONGEST) found.push(`${what} holds a run of ${m.longest}`);
  });
  if (terminal === null) return found;
  const extra = terminal.keys.filter((key) => !REPLY_KEYS.has(key));
  if (extra.length > 0) found.push(`the terminal reply carries fields nobody listed: ${extra}`);
  if (terminal.ports > 0) found.push(`the terminal reply transfers ${terminal.ports} port(s)`);
  if (terminal.ok === true && terminal.bytes !== 1) {
    found.push(`the successful reply carries ${terminal.bytes} byte values; exactly one may`);
  }
  if (terminal.ok === false && terminal.bytes !== 0) {
    found.push(`a refusal, and it carried ${terminal.bytes} byte value(s) anyway`);
  }
  return found;
}

/**
 * R9, HELD MORE STRICTLY THAN ADR 0006 WORDS IT. The ADR says no exit before verification; this
 * says NO EXIT AT ALL, from arming until the settle marker. Redaction's worker has no reason to
 * take one at any point -- the page makes the download link from the Blob it is handed -- and the
 * stricter rule is what catches an exit on a REFUSAL, where nothing was ever verified, and one a
 * timer deferred past the reply (review of #137). Returns every exit taken, or nothing.
 *
 * Deferred by more than `SETTLE_MS`, an exit is not seen; that bound is stated, not closed.
 */
export function r9Violations(all: RedactionMessage[]): string[] {
  const log = all.filter((m) => m.sent === null);
  const found = log
    .map((m, at) => ({ m, at }))
    .filter(({ m }) => m.sideChannel !== null)
    .map(({ m, at }) => `${m.sideChannel} was called at message ${at}`);
  if (!log.some((m) => m.keys.includes("__burrowSettled"))) {
    found.push("the log was read before the worker went quiet");
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
