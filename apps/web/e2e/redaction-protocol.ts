// What R8 and R9 say about redaction's worker, as functions of what it posted (#137).
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

/** Redact `WRITER`'s `page` (1-based) through redaction's worker, as armed. */
export async function redactWriter(page: Page, pageNumber: number): Promise<Reply> {
  return page.evaluate(
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
}

/** The id of the one redaction the page sent in `log`. Refuses if it did not send exactly one. */
export function onlyRequest(log: RedactionMessage[]): number {
  const ids = log.filter((m) => m.sent === "redact").map((m) => m.id);
  if (ids.length !== 1 || typeof ids[0] !== "number") {
    throw new Error(`expected one redaction request, found ${JSON.stringify(ids)}`);
  }
  return ids[0];
}

/** What the WORKER posted, in order: the half of `log` R8 and R9 are about. */
function received(log: RedactionMessage[]): RedactionMessage[] {
  return log.filter((m) => m.sent === null);
}

/**
 * R8: the output is ONE value, posted after the Rust call returned. Returns every way `log`
 * breaks it, or nothing.
 *
 * - No message WITHOUT a request id may carry bytes: a stray post is still a post.
 * - Before the terminal reply -- the last message for the request that has an `ok` -- no message
 *   for it may carry bytes.
 * - A successful terminal reply carries the document as exactly ONE byte-carrying value -- ADR
 *   0023's "one part, not merely one message" -- and it is the ONLY message that carries any. A
 *   refusal carries none.
 */
export function r8Violations(all: RedactionMessage[], id: number): string[] {
  const log = received(all);
  const found: string[] = [];
  log.forEach((m, at) => {
    if (m.id === null && m.bytes > 0)
      found.push(`message ${at} carries bytes and names no request`);
  });
  const mine = log.map((m, at) => ({ m, at })).filter(({ m }) => m.id === id);
  const replies = mine.filter(({ m }) => m.ok !== null);
  // NO TERMINAL REPLY IS ITSELF A FINDING, and it does not end the search: a message under the
  // request's id is what the host takes as the answer, so a chunk posted with the id resolves the
  // request and the real reply arrives after anyone looked (measured, on this spec's own mutation).
  const terminal = replies.length > 0 ? replies[replies.length - 1] : null;
  if (terminal === null) found.push("no terminal reply");
  for (const { m, at } of mine) {
    if ((terminal === null || at < terminal.at) && m.bytes > 0) {
      found.push(`message ${at} carries bytes before the terminal reply (keys ${m.keys})`);
    }
  }
  if (terminal === null) return found;
  const carrying = mine.filter(({ m }) => m.bytes > 0).length;
  if (terminal.m.ok === true) {
    if (terminal.m.bytes !== 1) {
      found.push(`the successful reply carries ${terminal.m.bytes} byte values; exactly one may`);
    }
    if (carrying !== 1) found.push(`${carrying} messages carry bytes; exactly one may`);
  } else if (carrying !== 0) {
    found.push(`a refusal, and ${carrying} message(s) carried bytes anyway`);
  }
  return found;
}

/**
 * R9: nothing leaves the heap before the verifier has run -- no `blob:` URL, no OPFS handle, no
 * save dialog -- so no stubbed exit may be called before the reply that carries the VERIFIED
 * document. Returns every way `log` breaks it, or nothing.
 */
export function r9Violations(all: RedactionMessage[], id: number): string[] {
  const log = received(all);
  const verified = log.findIndex((m) => m.id === id && m.ok === true && m.bytes > 0);
  if (verified === -1) return ["no verified reply carrying the document"];
  return log
    .map((m, at) => ({ m, at }))
    .filter(({ m, at }) => m.sideChannel !== null && at < verified)
    .map(({ m, at }) => `${m.sideChannel} was called at message ${at}, before the verified reply`);
}

/** The stubs the prologue reports it installed, from its first message. */
export function armedStubs(log: RedactionMessage[]): string[] {
  return log.find((m) => m.armed !== null)?.armed ?? [];
}
