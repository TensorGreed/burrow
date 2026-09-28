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
const STAGES: Record<string, string> = {
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

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/**
 * How `reply` diverges from `outcome`, or null when it does not. Every field the class compares
 * is named in the result, so a report says what differed rather than that something did.
 */
export function divergence(outcome: string, reply: Reply): string | null {
  const found: string[] = [];
  if (outcome.startsWith("OK ")) {
    const [, document, word, report] = outcome.split(" ");
    if (word !== "report") return `an OK outcome the golden file does not write: ${outcome}`;
    if (!reply.ok) return `native redacted it; the web refused, ${reply.kind}: ${reply.message}`;
    if (reply.outputSha256 !== document) found.push("document");
    if (sha256(reply.report ?? "") !== report) found.push("report");
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
  const actual = ruleOf(reply.message);
  if (expected !== actual)
    return `refused by rule ${actual ?? "(none)"}, not ${expected ?? "(none)"}`;
  return null;
}
