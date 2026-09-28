// #137's browser differential: every redaction outcome the native golden file pins, and how a
// web reply is held to it. The comparison lives here, apart from the spec, so the function a real
// run passes is the SAME function a planted divergence must fail.
//
// WHAT IS COMPARED, per outcome the golden file can hold:
// - `OK <sha256 of the document> report <sha256 of the Debug report>` -- both digests, exactly.
// - `ERR <Kind>(... [rule-name] ...)` -- the typed kind and the rule name in brackets. Both sides
//   write that name into the error at the point that decides it, so it is compared as a TOKEN,
//   never by mapping one side's prose onto the other's: a mapping derived from text is the shape
//   that rots (owner's direction on #137).
// - `ERR LimitExceeded { limit, stage, requested, allowed }` -- every field, each typed on the web
//   side (`limit`, `stage`, `requested`, `allowed`). `stage` is spelled as the `Debug` variant
//   natively and as `Stage::as_str` on the web; `STAGES` is that one enum's two spellings, copied
//   from `core/burrow-types/src/stage.rs`, and an unknown variant fails rather than passing.
// - `ERR PasswordRequired` -- no payload, so the kind is the whole outcome.
// - Any other refusal with NO rule name -- today `Malformed("qpdf: the document is damaged")`, an
//   engine-boundary refusal -- is compared by kind, and by the absence of a rule on BOTH sides.
//   That is weaker than the rest, and it is counted and reported as its own class rather than
//   folded into "matched".

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import type { Reply } from "../src/host/harness-api";

const here = dirname(fileURLToPath(import.meta.url));
export const repo = resolve(here, "../../..");

/** The regions `core/burrow-ops/tests/redaction_outcomes.rs` names, by label. */
const FIXED_REGIONS: Record<string, Region> = {
  pdfbuild: { left: 30, top: 68, width: 340, height: 44 },
  whole: { left: 0, top: 0, width: 5000, height: 5000 },
  band: { left: 0, top: 300, width: 5000, height: 120 },
};

export interface Region {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** One recorded case, as the golden file writes it, with its region resolved. */
export interface Case {
  /** 0-based, as recorded. */
  page: number;
  /** 0-based, as recorded. */
  covered: number[];
  label: string;
  region: Region;
  outcome: string;
}

/** One document the golden file records, and every case recorded against it. */
export interface Document {
  name: string;
  digest: string;
  cases: Case[];
}

/**
 * The manifest's `region` for each fixture that declares one, keyed as the golden file names the
 * document. Parsed as the native test parses it -- a `region` line precedes its `file` inside a
 * `[[fixture]]` -- and every declared region must be attributed, so a format change fails here
 * rather than quietly yielding fewer.
 */
export function manifestRegions(): Map<string, Region> {
  const text = readFileSync(join(repo, "tests/redaction/manifest.toml"), "utf8");
  const out = new Map<string, Region>();
  let region: Region | null = null;
  let declared = 0;
  const valueOf = (line: string, key: string): string | null => {
    const at = line.indexOf("=");
    if (at === -1 || line.slice(0, at).trim() !== key) return null;
    return line.slice(at + 1).trim();
  };
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (line === "[[fixture]]") {
      region = null;
      continue;
    }
    const value = valueOf(line, "region");
    if (value !== null) {
      declared += 1;
      const numbers = value
        .replace(/^\[|\]$/g, "")
        .split(",")
        .map(Number);
      if (numbers.length !== 4 || numbers.some((n) => !Number.isFinite(n))) {
        throw new Error(`a manifest region that is not four numbers: ${line}`);
      }
      const [left, top, width, height] = numbers;
      region = { left, top, width, height };
      continue;
    }
    const file = valueOf(line, "file");
    if (file !== null && region !== null) {
      out.set(`tests/redaction/${file.replace(/^"|"$/g, "")}`, region);
      region = null;
    }
  }
  if (out.size !== declared) {
    throw new Error(`the manifest declares ${declared} regions and ${out.size} were attributed`);
  }
  return out;
}

/** Every document and case `tests/redaction/outcomes.tsv` records, in its order. */
export function golden(): Document[] {
  const regions = manifestRegions();
  const documents: Document[] = [];
  const lines = readFileSync(join(repo, "tests/redaction/outcomes.tsv"), "utf8").split("\n");
  for (const line of lines) {
    const fields = line.split("\t");
    if (fields[0] === "INPUT") {
      documents.push({ name: fields[1], digest: fields[2], cases: [] });
    } else if (fields[0] === "CASE") {
      const [, name, page, covered, label, outcome] = fields;
      const document = documents[documents.length - 1];
      if (document === undefined || document.name !== name) {
        throw new Error(`a CASE for ${name} that does not follow its INPUT line`);
      }
      const region = label === "manifest" ? regions.get(name) : FIXED_REGIONS[label];
      if (region === undefined) throw new Error(`${name}: no region for the label ${label}`);
      document.cases.push({
        page: Number(page),
        covered: covered.split(",").map(Number),
        label,
        region,
        outcome,
      });
    }
  }
  return documents;
}

/** A document's bytes, held to the digest the golden file recorded, or why not. */
export function documentBytes(document: Document): { bytes: Buffer } | { problem: string } {
  let bytes: Buffer;
  try {
    bytes = readFileSync(join(repo, document.name));
  } catch {
    return { problem: `${document.name} is not on disk` };
  }
  const digest = createHash("sha256").update(bytes).digest("hex");
  return digest === document.digest
    ? { bytes }
    : { problem: `${document.name} is not the document the golden file recorded (${digest})` };
}

/** `Stage`'s two spellings: the `Debug` variant the golden file holds, and `as_str`. */
export const STAGES: Record<string, string> = {
  InputSize: "input_size",
  SizeEstimate: "size_estimate",
  Prescan: "prescan",
  PageCount: "page_count",
  Pixels: "pixels",
  Measured: "measured",
  Deadline: "deadline",
};

/** The rule name an error carries in brackets, or null for one that carries none. */
export function ruleOf(text: string): string | null {
  return /\[([a-z0-9-]+)\]/.exec(text)?.[1] ?? null;
}

/** Which comparison a recorded outcome gets: the classes the spec counts and reports. */
export type Class = "document" | "rule" | "limit" | "kind";

export function classOf(outcome: string): Class {
  if (outcome.startsWith("OK ")) return "document";
  if (outcome.startsWith("ERR LimitExceeded {")) return "limit";
  return ruleOf(outcome) === null ? "kind" : "rule";
}

// ---- THE LEDGER: how many of the golden file's cases were compared, and why the rest were not ---
//
// "every test passed" is a count of TESTS, and a test can pass having compared fewer cases than
// the golden file pins: a filter in the loop, a slice in the plan, a parser that drops a label.
// Measured on this spec before the ledger existed -- a status note recorded "372/372", which was
// the test count of an earlier revision, and read as a case count 91 short of 463. So each
// document test records, for each case it ran, what the web OBSERVED -- `Observed`, the fields the
// comparison reads and nothing else -- and the ledger JUDGES it: against the outcome the golden
// file records under that case's name, read a SECOND time by `goldenCases` below, independently of
// `golden()`. A case `golden()` loses is still expected; a verdict the document test computed
// against the wrong case, or parked, is not the ledger's verdict.
//
// The ledger trusts one thing: that an observation came from the worker. A document test that
// FABRICATED observations to match the golden file would pass it, and nothing here can tell.

/** A case's name in every report: the document, then what the golden file says of the case. */
export function caseName(
  document: string,
  c: { page: number | string; covered: number[] | string; label: string },
): string {
  const covered = Array.isArray(c.covered) ? c.covered.join(",") : c.covered;
  return `${document} page ${c.page} covering ${covered}, ${c.label}`;
}

/** What the golden file records of one case: the outcome, and which comparison it gets. */
export interface Expected {
  class: Class;
  outcome: string;
}

/**
 * Every case `tests/redaction/outcomes.tsv` records, by name, with its outcome and class -- read
 * line by line and NOTHING ELSE: no region lookup, no document grouping, nothing `golden()` does
 * that could drop one. Names are unique in the file today; a duplicate fails here, because the
 * ledger could not tell two cases of one name apart.
 */
export function goldenCases(): Map<string, Expected> {
  const out = new Map<string, Expected>();
  const text = readFileSync(join(repo, "tests/redaction/outcomes.tsv"), "utf8");
  for (const line of text.split("\n")) {
    if (!line.startsWith("CASE\t")) continue;
    const [, name, page, covered, label, outcome] = line.split("\t");
    const key = caseName(name, { page, covered, label });
    if (out.has(key)) throw new Error(`the golden file records ${key} twice`);
    out.set(key, { class: classOf(outcome), outcome });
  }
  return out;
}

/**
 * A case, or a whole document, the differential does not compare, and why.
 *
 * EMPTY, AND THAT IS A MEASUREMENT: on 2026-09-28 every case the golden file records was compared
 * in all three browsers. What is weaker is not skipped -- the rule-less refusals are compared by
 * kind and reported as their own class (`kind`) -- and the generated documents the `web` job cannot
 * build arrive as `test`'s artifact rather than being left out. An entry here names a document
 * exactly as the golden file does, and, for one case, `caseName`'s spelling of it. A declaration
 * that names nothing the golden file records, or that has no reason, fails the ledger.
 */
export interface Skip {
  document: string;
  /** `caseName(...)` of the one case; absent, every case of the document. */
  case?: string;
  reason: string;
}

export const DECLARED_SKIPS: readonly Skip[] = [];

/** The cases of `document` the differential runs: every one not declared skipped. */
export function planned(document: Document, skips: readonly Skip[] = DECLARED_SKIPS): Case[] {
  return document.cases.filter(
    (c) =>
      !skips.some(
        (s) =>
          s.document === document.name &&
          (s.case === undefined || s.case === caseName(document.name, c)),
      ),
  );
}

/** What a document test records of one case: its name, and what the web was observed to reply. */
export interface Recorded {
  name: string;
  observed: Observed;
}

/**
 * The ledger against the golden file: every case compared exactly once AND AGREEING -- judged here,
 * against this module's own read of its outcome -- or declared skipped with a reason, and nothing
 * else. `problems` names each disagreement; `summary` is what was examined, against what should
 * have been, and why the difference.
 */
export function reconcile(
  expected: Map<string, Expected>,
  compared: Recorded[],
  skips: readonly Skip[] = DECLARED_SKIPS,
): { problems: string[]; summary: string } {
  const problems: string[] = [];
  const skipped = new Map<string, string>();
  for (const s of skips) {
    const label = s.case ?? `${s.document} (every case)`;
    if (s.reason.trim() === "") problems.push(`declared skipped with no reason: ${label}`);
    const covers = [...expected.keys()].filter((k) =>
      s.case === undefined ? k.startsWith(`${s.document} page `) : k === s.case,
    );
    if (covers.length === 0 || (s.case !== undefined && !s.case.startsWith(`${s.document} page `)))
      problems.push(`declared skipped, and the golden file records no such case: ${label}`);
    for (const k of covers) skipped.set(k, s.reason);
  }
  const seen = new Map<string, number>();
  const diverged = new Set<string>();
  for (const { name, observed } of compared) {
    seen.set(name, (seen.get(name) ?? 0) + 1);
    // THE LEDGER'S OWN VERDICT, against its own read of the outcome: a document test can be
    // parked (`test.fail()`, a runtime `test.skip`) or can judge against the wrong case, and
    // still read as green. Parking belongs in `DECLARED_SKIPS`, with a reason.
    const outcome = expected.get(name)?.outcome;
    if (outcome === undefined) continue;
    const why = isObserved(observed)
      ? judge(outcome, observed)
      : "a record that is not an observation";
    if (why !== null) {
      diverged.add(name);
      problems.push(`compared, and diverged: ${name}: ${why}`);
    }
  }
  for (const [k, n] of seen) {
    if (!expected.has(k)) problems.push(`compared, and not in the golden file: ${k}`);
    else if (n > 1) problems.push(`compared ${n} times: ${k}`);
    if (skipped.has(k)) problems.push(`declared skipped, and compared: ${k}`);
  }
  for (const k of expected.keys()) {
    if (!seen.has(k) && !skipped.has(k))
      problems.push(`not compared, and not declared skipped: ${k}`);
  }

  const classes = new Map<Class, number>();
  let counted = 0;
  for (const k of seen.keys()) {
    const c = expected.get(k)?.class;
    if (c === undefined) continue;
    counted += 1;
    classes.set(c, (classes.get(c) ?? 0) + 1);
  }
  const breakdown = (["document", "rule", "limit", "kind"] as const)
    .map((c) => `${classes.get(c) ?? 0} ${c === "kind" ? "by kind only" : `by ${c}`}`)
    .join(", ");
  // EVERY SKIP BY NAME, with its reason: a bare count of skips is the report this replaces.
  const declared = skips.map((s) => {
    const n = [...skipped.keys()].filter((k) =>
      s.case === undefined ? k.startsWith(`${s.document} page `) : k === s.case,
    ).length;
    return `\n  skipped ${n}: ${s.case ?? `${s.document} (every case)`} -- ${s.reason}`;
  });
  const summary =
    `${expected.size} cases in the golden file: ${counted} compared (${breakdown}), ` +
    `${diverged.size} diverged, ${skipped.size} skipped${declared.join("")}`;
  return { problems, summary };
}

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/**
 * What the comparison reads of a reply, and nothing else: no prose, no bytes. The refusal's
 * message is reduced to its rule name and the report to its digest here, so a ledger line --
 * written to disk -- carries no text an engine produced.
 */
export interface Observed {
  ok: boolean;
  kind: string;
  rule: string | null;
  limit: string;
  stage: string;
  requested: string;
  allowed: string;
  outputSha256: string | null;
  reportSha256: string | null;
}

export function observe(reply: Reply): Observed {
  return {
    ok: reply.ok,
    kind: reply.kind,
    rule: ruleOf(reply.message),
    limit: reply.limit,
    stage: reply.stage,
    requested: reply.requested,
    allowed: reply.allowed,
    outputSha256: reply.outputSha256 ?? null,
    reportSha256: reply.ok ? sha256(reply.report ?? "") : null,
  };
}

/**
 * Every field `Observed` has, each of its type. Extra fields are not refused, and need not be:
 * `judge` reads only these, by exact equality.
 */
function isObserved(value: unknown): value is Observed {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  const text = (k: string) => typeof v[k] === "string";
  const textOrNull = (k: string) => typeof v[k] === "string" || v[k] === null;
  return (
    typeof v.ok === "boolean" &&
    ["kind", "limit", "stage", "requested", "allowed"].every(text) &&
    ["rule", "outputSha256", "reportSha256"].every(textOrNull)
  );
}

/**
 * How `reply` diverges from `outcome`, or null when it does not. Every field the class compares
 * is named in the result, so a report says what differed rather than that something did.
 */
export function divergence(outcome: string, reply: Reply): string | null {
  return judge(outcome, observe(reply));
}

/** `divergence`, over what was observed: the one comparison the tests and the ledger both make. */
export function judge(outcome: string, reply: Observed): string | null {
  const found: string[] = [];
  if (outcome.startsWith("OK ")) {
    const [, document, word, report] = outcome.split(" ");
    if (word !== "report") return `an OK outcome the golden file does not write: ${outcome}`;
    if (!reply.ok) return `native redacted it; the web refused, ${reply.kind} [${reply.rule}]`;
    if (reply.outputSha256 !== document) found.push("document");
    if (reply.reportSha256 !== report) found.push("report");
    return found.length === 0 ? null : `a different ${found.join(" and ")}`;
  }

  const kind = /^ERR ([A-Za-z]+)/.exec(outcome)?.[1];
  if (kind === undefined) return `an outcome the golden file does not write: ${outcome}`;
  if (reply.ok) return `native refused it (${kind}); the web redacted it`;
  if (reply.kind !== kind) return `native refused it as ${kind}; the web as ${reply.kind}`;

  if (outcome.startsWith("ERR LimitExceeded {")) {
    const limit = /limit: "([a-z_]+)"/.exec(outcome)?.[1];
    const stage = /stage: ([A-Za-z]+)/.exec(outcome)?.[1];
    const requested = /requested: (\d+)/.exec(outcome)?.[1];
    const allowed = /allowed: (\d+)/.exec(outcome)?.[1];
    if ([limit, stage, requested, allowed].includes(undefined)) {
      return `a LimitExceeded the golden file does not write: ${outcome}`;
    }
    const expectedStage = STAGES[stage as string];
    if (expectedStage === undefined) return `a stage STAGES does not name: ${stage}`;
    if (reply.limit !== limit) found.push(`limit ${reply.limit}, not ${limit}`);
    if (reply.stage !== expectedStage) found.push(`stage ${reply.stage}, not ${expectedStage}`);
    if (reply.requested !== requested) found.push(`requested ${reply.requested}, not ${requested}`);
    if (reply.allowed !== allowed) found.push(`allowed ${reply.allowed}, not ${allowed}`);
    return found.length === 0 ? null : `a different ceiling: ${found.join(", ")}`;
  }

  const expected = ruleOf(outcome);
  const actual = reply.rule;
  if (expected !== actual)
    return `refused by rule ${actual ?? "(none)"}, not ${expected ?? "(none)"}`;
  return null;
}
