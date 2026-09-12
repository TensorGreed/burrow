// The differential comparison, as a pure function.
//
// WHY THIS IS NOT INSIDE THE PLAYWRIGHT SPEC
//
// Every check in this project that *could* report success while checking nothing eventually
// did: a fake that returned a fixed handle and hid a leak, a CSP assertion that passed because
// the worker was never asked, a recovery test that could not fail. A differential harness is
// the most tempting shape for that failure, because "no divergences found" and "no comparisons
// made" render identically.
//
// So the comparison takes data and returns a verdict, with no browser, no filesystem and no
// network in it. `compare.test.ts` then drives it with planted divergences — one on each side,
// because they take different branches — and with every degenerate corpus the defences below
// exist for. A defence that is only exercised end-to-end is a defence nobody has seen fail.
//
// WHAT IT COMPARES, AND WHY THAT IS MORE THAN THE GOLDEN FILE
//
// Both implementations already assert themselves against `expectations.json`, which is
// transitive: if native matches and web matches, native matches web. That catches both paths
// being wrong in the same way, which a direct diff cannot.
//
// It is also only as complete as the schema. This diffs the two records against **each other**,
// so a divergence in something nobody thought to record still fails.

/** A `burrow_types::Error` variant, by name. Mirrors `ErrorKind` in `expectations.rs`. */
export type ErrorKind =
  | "Malformed"
  | "Unsupported"
  | "PasswordRequired"
  | "LimitExceeded"
  | "InvalidArgument"
  | "Io"
  | "Internal";

/** A typed failure, in as much detail as is comparable across implementations. */
export interface Failure {
  kind: ErrorKind;
  limit?: string;
  /** Which check fired. See `burrow_types::Stage`. */
  stage?: string;
  /** Omitted at the `measured` stage — the two platforms count different things. */
  requested?: number;
  allowed?: number;
}

export type Outcome = { ok: { page_count: number } } | { err: Failure };

export type Operation = "page_count" | "structure_check";
export type Platform = "native" | "web";

export interface PlatformExpectation {
  platform: Platform;
  operation: Operation;
  expect: Outcome;
  reason: string;
}

export interface KnownGap {
  issue: string;
  reason: string;
  /**
   * The milestone this gap must be closed by, e.g. `"M2"`.
   *
   * Enforced on the Rust side, against `Expectations.current_milestone` -- there, and not
   * here, because governance of the corpus belongs in one place and the native suite is the
   * one CI asserts by name actually ran.
   *
   * Declaring it here buys **nothing at runtime**, and an earlier version of this comment
   * claimed otherwise. `conformance.spec.ts` reads the file with
   * `JSON.parse(raw) as Expectations`, an unchecked assertion, so an `expectations.json`
   * without this field typechecks and renders "due by undefined". The type is a statement of
   * the shape for readers and for code that constructs one, not a gate.
   */
  milestone: string;
}

export interface CaseLimits {
  max_input_bytes?: number;
  max_memory_bytes?: number;
  max_duration_ms?: number;
  max_pages?: number;
}

export interface Case {
  name: string;
  file: string;
  sha256: string;
  password: string | null;
  limits?: CaseLimits;
  attempt_recovery?: boolean;
  expect: { page_count: Outcome; structure_check: Outcome };
  platform_expectations?: PlatformExpectation[];
  known_gap?: KnownGap;
}

export interface Expectations {
  schema: number;
  generated_by: string;
  /** The milestone burrow is working in. See {@link KnownGap.milestone}. */
  current_milestone: string;
  cases: Case[];
}

export interface RecordedOutcome {
  case: string;
  operation: Operation;
  outcome: Outcome;
}

export interface OutcomeRecord {
  platform: Platform;
  runner: string;
  expectations_sha256: string;
  results: RecordedOutcome[];
}

/** One accepted-but-not-designed engine difference. See `tests/conformance/divergences.toml`. */
export interface Divergence {
  case: string;
  operation: Operation;
  reason: string;
  issue: string;
}

export interface Verdict {
  /** How many case × operation pairs were actually compared. */
  comparisons: number;
  /** Divergences that no rule accounts for. Any entry is a failure. */
  failures: string[];
  /** Allowlisted divergences that occurred, so they can be reported rather than hidden. */
  allowed: string[];
  /** Known gaps this corpus records, for the run summary. */
  gaps: string[];
  /** Allowlist entries whose divergence did not happen. Also a failure: the record is stale. */
  stale: string[];
}

/** The schema this comparator understands. A newer file must fail, not be guessed at. */
export const SUPPORTED_SCHEMA = 2;

/**
 * Render an outcome for a failure message.
 *
 * **Typed detail only** — a kind, a limit, a stage, and numbers the caller themselves set.
 * Never fixture bytes, never an engine's message, never a canary. `secret_leak.rs`'s
 * discipline applies to CI logs, and a conformance failure is exactly the moment somebody
 * pastes one into an issue. `compare.test.ts` asserts this directly.
 */
export function render(outcome: Outcome | undefined): string {
  if (outcome === undefined) return "<no result>";
  if ("ok" in outcome) return `ok(${outcome.ok.page_count} pages)`;
  const f = outcome.err;
  const detail = [
    f.limit === undefined ? null : `limit=${f.limit}`,
    f.stage === undefined ? null : `stage=${f.stage}`,
    f.requested === undefined ? null : `requested=${f.requested}`,
    f.allowed === undefined ? null : `allowed=${f.allowed}`,
  ].filter((part): part is string => part !== null);
  return detail.length === 0 ? f.kind : `${f.kind}(${detail.join(" ")})`;
}

/**
 * Structural equality, with keys sorted first.
 *
 * `JSON.stringify` alone is key-order sensitive, and two value-identical outcomes whose keys
 * were emitted in different orders would compare unequal — producing a failure whose "expected"
 * and "actual" lines *render identically*, which is about the least actionable red a test can
 * be. It works today only because serde's field order, `outcomeOf`'s literal order and the
 * committed JSON happen to coincide; that is a coincidence, not a guarantee, and the two sides
 * are maintained in different languages.
 */
function canonical(value: unknown): string {
  return JSON.stringify(value ?? null, (_key, v: unknown) => {
    if (v === null || typeof v !== "object" || Array.isArray(v)) return v;
    const sorted: Record<string, unknown> = {};
    for (const key of Object.keys(v as Record<string, unknown>).sort()) {
      sorted[key] = (v as Record<string, unknown>)[key];
    }
    return sorted;
  });
}

function sameOutcome(a: Outcome | undefined, b: Outcome | undefined): boolean {
  return canonical(a) === canonical(b);
}

function key(caseName: string, operation: Operation): string {
  return `${caseName}::${operation}`;
}

const OPERATIONS: Operation[] = ["page_count", "structure_check"];

/**
 * What a case expects of one platform, honouring any recorded by-design difference.
 *
 * Mirrors `expected_for` in `core/burrow-engines/tests/conformance.rs`. Two implementations of
 * one rule, which is what `expectations.json` exists to avoid — but this one cannot be shared,
 * because the two halves of the harness are in different languages. It is four lines, it is
 * tested on both sides, and the alternative is the web side not honouring recorded differences
 * at all.
 */
export function expectedFor(c: Case, platform: Platform, operation: Operation): Outcome {
  const recorded = (c.platform_expectations ?? []).find(
    (p) => p.platform === platform && p.operation === operation,
  );
  if (recorded) return recorded.expect;
  return operation === "page_count" ? c.expect.page_count : c.expect.structure_check;
}

/**
 * Compare one implementation's record against the corpus, and against the other's.
 *
 * Returns a verdict rather than throwing, so the caller decides how to report it and so the
 * tests can inspect every field. `failures` and `stale` non-empty both mean CI fails.
 */
export function compare(
  expectations: Expectations,
  native: OutcomeRecord,
  web: OutcomeRecord,
  allowlist: Divergence[],
  expectationsDigest: string,
): Verdict {
  const failures: string[] = [];
  const allowed: string[] = [];
  const gaps: string[] = [];
  const stale: string[] = [];
  let comparisons = 0;

  if (expectations.schema !== SUPPORTED_SCHEMA) {
    failures.push(
      `expectations.json uses schema ${expectations.schema}; this comparator understands ` +
        `${SUPPORTED_SCHEMA}. A reader that guessed would compare the wrong fields.`,
    );
    return { comparisons, failures, allowed, gaps, stale };
  }

  // THE TWO RECORDS MUST BE THE TWO PLATFORMS.
  //
  // Found by both reviewers, independently, and each demonstrated it: `compare(exp, web, web,
  // [])` used to pass clean with a full comparison count. `platform` was in the type, present
  // in both records, and read only to render a failure message. That is exactly the "no
  // divergences found and no comparisons made render identically" failure this file's header
  // is written against, surviving in the one field that rules it out for two lines.
  if (native.platform !== "native" || web.platform !== "web") {
    failures.push(
      `the two records are ${native.platform} and ${web.platform}; a differential comparison ` +
        `needs one of each, and comparing a record with itself agrees perfectly`,
    );
    return { comparisons, failures, allowed, gaps, stale };
  }

  // FRESHNESS, against an INDEPENDENT digest.
  //
  // An earlier version compared each record to `web.expectations_sha256`, which made the `web`
  // iteration a no-op and asserted only that the two agreed with each other — so two records
  // carrying the same *wrong* digest passed. The digest is now supplied by the caller, computed
  // over the bytes it parsed, so a record written against a different corpus fails here rather
  // than comparing cleanly against files it never ran.
  for (const record of [native, web]) {
    if (record.expectations_sha256 !== expectationsDigest) {
      failures.push(
        `${record.platform}/${record.runner} was written against expectations digest ` +
          `${record.expectations_sha256}, and this corpus is ${expectationsDigest} — that ` +
          `record is stale`,
      );
    }
  }

  // DUPLICATE ROWS. `new Map(results.map(...))` is last-wins, so a record carrying a wrong row
  // followed by a right one for the same key would compare clean. Counted rather than
  // deduplicated: a record with the wrong number of rows is not a record to reason about.
  for (const record of [native, web]) {
    const unique = new Set(record.results.map((r) => key(r.case, r.operation)));
    if (unique.size !== record.results.length) {
      failures.push(
        `${record.platform}/${record.runner} has ${record.results.length} rows but only ` +
          `${unique.size} distinct (case, operation) pairs; duplicates are silently last-wins`,
      );
    }
  }

  if (expectations.cases.length === 0) {
    failures.push("the corpus is empty, so every comparison below would be vacuous");
  }

  const nativeBy = new Map(native.results.map((r) => [key(r.case, r.operation), r.outcome]));
  const webBy = new Map(web.results.map((r) => [key(r.case, r.operation), r.outcome]));
  const matchedAllowlist = new Set<string>();

  for (const c of expectations.cases) {
    if (c.known_gap) {
      gaps.push(`${c.name}: ${c.known_gap.issue} (due by ${c.known_gap.milestone})`);
    }

    for (const operation of OPERATIONS) {
      const k = key(c.name, operation);
      const n = nativeBy.get(k);
      const w = webBy.get(k);

      // BOTH PATHS RAN. A missing entry is never agreement: an implementation that errored
      // out before reaching a case produces no row for it, and two records that both omit a
      // case would otherwise compare equal.
      if (n === undefined || w === undefined) {
        failures.push(
          `${k}: ${n === undefined ? "native" : "web"} produced no result. One path not ` +
            `running is not the same as the two paths agreeing.`,
        );
        continue;
      }

      comparisons += 1;

      // Against the corpus, per platform — this is what catches both paths being wrong the
      // same way, which comparing them to each other never can.
      const expectedNative = expectedFor(c, "native", operation);
      const expectedWeb = expectedFor(c, "web", operation);
      if (!sameOutcome(n, expectedNative)) {
        failures.push(
          `${k}: native did not match the corpus\n    expected ${render(expectedNative)}\n` +
            `    actual   ${render(n)}\n    fixture  ${c.file} sha256 ${c.sha256}`,
        );
      }
      if (!sameOutcome(w, expectedWeb)) {
        failures.push(
          `${k}: web (${web.runner}) did not match the corpus\n    expected ` +
            `${render(expectedWeb)}\n    actual   ${render(w)}\n    fixture  ${c.file} ` +
            `sha256 ${c.sha256}`,
        );
      }

      if (sameOutcome(n, w)) {
        // Equal. If a rule said they should differ, that rule is now stale — a recorded
        // difference that has silently gone away is as much a problem as an unrecorded one,
        // because the record is what the next reader will believe.
        const rule = (c.platform_expectations ?? []).find((p) => p.operation === operation);
        if (rule) {
          stale.push(
            `${k}: a platform expectation records that ${rule.platform} differs here, and it ` +
              `no longer does. Remove it deliberately rather than leaving it to be believed.`,
          );
        }
        const waiver = allowlist.find((d) => d.case === c.name && d.operation === operation);
        if (waiver) {
          stale.push(
            `${k}: an allowlisted divergence (${waiver.issue}) no longer occurs. Remove the ` +
              `entry and close the issue.`,
          );
        }
        continue;
      }

      // They differ. Exactly one of two things may excuse it.
      const byDesign = (c.platform_expectations ?? []).find((p) => p.operation === operation);
      if (byDesign) {
        // Already checked against the corpus above; the difference is the design.
        continue;
      }

      const waiver = allowlist.find((d) => d.case === c.name && d.operation === operation);
      if (waiver) {
        matchedAllowlist.add(k);
        allowed.push(`${k}: ${waiver.reason} (${waiver.issue})`);
        continue;
      }

      failures.push(
        `${k}: the two implementations disagree\n    native ${render(n)}\n` +
          `    web (${web.runner}) ${render(w)}\n    fixture ${c.file} sha256 ${c.sha256}`,
      );
    }
  }

  // An allowlist entry for a case that is not in the corpus is not a waiver, it is a typo that
  // silently waives nothing.
  const caseNames = new Set(expectations.cases.map((c) => c.name));
  for (const entry of allowlist) {
    if (!caseNames.has(entry.case)) {
      failures.push(
        `the divergence allowlist names ${entry.case}, which is not in the corpus. An entry ` +
          `that matches nothing waives nothing.`,
      );
    } else if (!matchedAllowlist.has(key(entry.case, entry.operation))) {
      // Covered by the stale check above when the outcomes are equal; this catches an entry
      // whose case exists but whose operation never produced a divergence at all.
      if (!stale.some((s) => s.startsWith(key(entry.case, entry.operation)))) {
        stale.push(
          `${key(entry.case, entry.operation)}: an allowlisted divergence (${entry.issue}) ` +
            `never occurred.`,
        );
      }
    }
  }

  // THE COUNT, last, so it reflects what actually happened. Zero comparisons is the failure
  // mode this whole file is shaped around: it looks exactly like success.
  const expected = expectations.cases.length * OPERATIONS.length;
  if (comparisons === 0) {
    failures.push("no comparisons were made at all");
  } else if (comparisons < expected) {
    failures.push(
      `only ${comparisons} of ${expected} comparisons were made; a skipped case is a failure ` +
        `unless it is a declared platform expectation`,
    );
  }

  return { comparisons, failures, allowed, gaps, stale };
}

/**
 * Parse the allowlist.
 *
 * Deliberately a hand-rolled reader for a tiny TOML subset rather than a dependency: the file
 * starts empty and every entry is four known keys. `apps/web/CLAUDE.md` has no TOML parser and
 * adding one to read a file that is usually empty is the wrong trade.
 *
 * **Every field is mandatory.** An entry missing a reason or an issue is not a waiver, it is a
 * divergence someone did not want to explain, and it fails rather than being ignored.
 */
export function parseAllowlist(text: string): { entries: Divergence[]; errors: string[] } {
  const entries: Divergence[] = [];
  const errors: string[] = [];
  let current: Partial<Divergence> | null = null;
  let line = 0;

  const finish = () => {
    if (!current) return;
    const missing = (["case", "operation", "reason", "issue"] as const).filter(
      (field) => current?.[field] === undefined,
    );
    if (missing.length > 0) {
      errors.push(`a [[divergence]] entry is missing: ${missing.join(", ")}`);
    } else {
      entries.push(current as Divergence);
    }
    current = null;
  };

  for (const raw of text.split("\n")) {
    line += 1;
    const trimmed = raw.trim();
    if (trimmed === "" || trimmed.startsWith("#")) continue;
    if (trimmed === "[[divergence]]") {
      finish();
      current = {};
      continue;
    }
    // `[^"]*` rather than `.*`: the greedy form swallowed anything after the closing quote, so
    // `issue = "https://x/1" # todo` parsed as the issue `https://x/1" # todo` and still passed
    // the `startsWith("https://")` check. A waiver whose issue link is not a link is not a
    // waiver.
    const match = /^(case|operation|reason|issue)\s*=\s*"([^"]*)"$/.exec(trimmed);
    if (!match) {
      errors.push(`line ${line}: not a recognised allowlist entry: ${trimmed}`);
      continue;
    }
    if (!current) {
      errors.push(`line ${line}: a key outside any [[divergence]] block`);
      continue;
    }
    const [, field, value] = match;
    if (field === "operation" && value !== "page_count" && value !== "structure_check") {
      errors.push(`line ${line}: ${value} is not an operation`);
      continue;
    }
    // Real TOML rejects a duplicate key; a hand-rolled reader that takes the last one would
    // apply a waiver to a case the first line of the block does not name.
    if ((current as Record<string, string | undefined>)[field] !== undefined) {
      errors.push(`line ${line}: ${field} is given twice in one [[divergence]] block`);
      continue;
    }
    (current as Record<string, string>)[field] = value;
  }
  finish();

  for (const entry of entries) {
    if (!entry.issue.startsWith("https://")) {
      errors.push(`${entry.case}: an allowlisted divergence must link to an issue`);
    }
    if (entry.reason.length < 40) {
      errors.push(`${entry.case}: an allowlisted divergence needs a reason, not a label`);
    }
  }

  return { entries, errors };
}
