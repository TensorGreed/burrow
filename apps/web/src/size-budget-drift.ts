// Does `size-budget.json` still describe the build it says it measured?
//
// THE HOLE THIS CLOSES, AND IT IS A REAL ONE THAT WENT UNNOTICED
//
// The budget tests gate two things: live sizes against `budget_brotli`, and the recorded
// lines against each other (the artifact figures must sum to the total). Both pass while
// the recorded `measured_*` values describe a build that no longer exists — because a
// recording that drifts *consistently* satisfies the sum, and the live-versus-budget gate
// never looks at the recording at all.
//
// Measured: `engines/burrow-worker.js` was recorded at 255,535 raw / 45,052 brotli and
// measured 256,255 / 45,288 on an **unchanged checkout**. 236 brotli bytes, from a
// differently-built `bindings/burrow-wasm/pkg/`, which is gitignored. Nothing failed.
// M1 PR A corrected the number; a corrected number is not a control, because the next
// drift is silent in exactly the same way.
//
// THE PROBE: SAME BYTES MUST MEAN SAME RECORDED SIZE
//
// So the recording gains a digest per artifact, and the rule becomes a contradiction the
// check can see:
//
//   * digest matches  → the recorded sizes MUST match exactly. Identical bytes with
//     different recorded numbers is not drift, it is a false record.
//   * digest differs  → the artifact was genuinely rebuilt. That is only acceptable for
//     artifacts declared as not byte-reproducible, with a reason — so an engine changing
//     underneath us fails rather than being waved through as "probably the toolchain".
//
// WHAT IS STILL NOT COVERED, SAID PLAINLY
//
// An artifact on the not-byte-reproducible list has no exact check, because there is
// nothing stable to compare against: its bytes legitimately differ per machine. That is
// where the 236 bytes hid, and the honest answer is a bound rather than a claim — its size
// must stay within `drift_tolerance` of the recording, so gross drift still fails and the
// invisible window is a declared width rather than an open door. The width is around the
// RECORDING, so changes that land unrecorded spend it between them: two that each pass can
// fail together, and the second is the one that sees it. Shortening that list is
// the real fix, and it means making `pkg/` reproducible, which is a build change and not
// this one.
//
// There is deliberately no "this exemption is unnecessary" finding. It was written and then
// removed: on the machine that took the recording every digest matches by construction, so
// it would have fired on every exempt artifact, always, for the one person best placed to
// re-record. A rule that cannot tell "reproducible" from "you are standing where the
// recording was made" is not a rule about reproducibility.

/** What the budget file records for one artifact. */
export interface Recorded {
  measured_raw: number;
  measured_brotli: number;
  /** Digest of the artifact's bytes when the sizes above were taken. */
  measured_sha256?: string;
  /** The same, with generated engine content hashes replaced by a fixed token. */
  measured_sha256_normalised?: string;
  /**
   * A base line's brotli allowance on an architecture other than the recording's (DECISIONS
   * rule 18, 2026-10-18 amendment): twice the largest cross-architecture difference measured
   * from saved runs. Read only for base lines, and only off the recording's architecture.
   */
  cross_arch_allowance_brotli?: number;
}

/** What the build actually contains for one artifact. */
export interface Live {
  raw: number;
  brotli: number;
  sha256: string;
  sha256Normalised: string;
}

export type Finding =
  | { kind: "no-digest"; key: string }
  | { kind: "false-record"; key: string; field: "raw" | "brotli"; recorded: number; live: number }
  | { kind: "undeclared-rebuild"; key: string }
  | { kind: "drift"; key: string; recorded: number; live: number; tolerance: number }
  | { kind: "hash-coupled-drift"; key: string; recorded: number; live: number; bound: number }
  | { kind: "base-drift"; key: string; recorded: number; live: number }
  | { kind: "no-allowance"; key: string }
  | { kind: "cross-arch-drift"; key: string; recorded: number; live: number; allowance: number };

/**
 * How far a size may move when only the generated engine hashes changed.
 *
 * MEASURED, not guessed: the page moved 2 bytes between a local build and CI, from the
 * engine URLs in its CSP compressing differently. 64 leaves margin for more engines
 * without being large enough to hide a real edit -- a paragraph of prose is bigger.
 */
export const HASH_COUPLING_BYTES = 64;

export interface Inputs {
  recorded: Record<string, Recorded>;
  live: Record<string, Live>;
  /** Artifact key to the reason its bytes cannot be reproduced from committed inputs. */
  notByteReproducible: Record<string, string>;
  /** Fractional size movement tolerated for those, e.g. `0.02`. */
  tolerance: number;
  /**
   * Present for BASE lines -- anything a visitor loads, the render lines included (DECISIONS
   * rule 18, 2026-10-18 amendment) -- and absent for held bundles, which keep `tolerance`.
   *
   * On the recording's architecture (`sameArch`) a base line may not move at all: every case
   * that would have taken `tolerance` or the 64-byte hash-coupling bound takes 0 instead. Off
   * it, each line moves at most its own `cross_arch_allowance_brotli`, and passing that is a
   * stop rather than a re-record: the allowance is a measured property of two toolchains, so a
   * line past it has changed in a way the recording architecture did not see.
   */
  base?: { sameArch: boolean };
}

/**
 * Compare a recording against a build.
 *
 * A pure function so the rules can be fed a planted stale recording on every run, rather
 * than only ever being exercised against the real files where they pass. A check nobody
 * has seen fail is one nobody knows works — the same argument `tokens-check.ts` makes.
 */
export function driftFindings({
  recorded,
  live,
  notByteReproducible,
  tolerance,
  base,
}: Inputs): Finding[] {
  const findings: Finding[] = [];

  for (const [key, record] of Object.entries(recorded)) {
    const actual = live[key];
    // A key with no live counterpart is already covered by the "budgets every artifact the
    // payload contains" test, in both directions. Not repeated here.
    if (actual === undefined) continue;

    if (!record.measured_sha256 || !record.measured_sha256_normalised) {
      // Without both digests there is nothing to compare bytes against, so every rule below
      // is vacuous for this line. That must fail rather than pass quietly: a missing digest
      // is how this check would be disabled by accident.
      findings.push({ kind: "no-digest", key });
      continue;
    }

    // THREE CASES, DISTINGUISHED RATHER THAN COLLAPSED. Collapsing the middle one into the
    // first is the bug CI caught: a normalised match is not identical bytes, so it cannot
    // carry an exact size claim.
    const sameBytes =
      record.measured_sha256 === actual.sha256 &&
      record.measured_sha256_normalised === actual.sha256Normalised;

    if (record.measured_sha256_normalised !== actual.sha256Normalised) {
      // The artifact's own content changed, not just the engine hashes quoted inside it.
      if (!(key in notByteReproducible)) {
        findings.push({ kind: "undeclared-rebuild", key });
        continue;
      }
    }

    if (base !== undefined && !sameBytes) {
      // A BASE LINE WHOSE BYTES DIFFER FROM THE RECORDING. No percentage and no 64-byte bound:
      // on the recording's architecture it may not move at all, and off it only by its own
      // measured allowance. Identical bytes fall through to the false-record check below.
      const moved = Math.abs(actual.brotli - record.measured_brotli);
      if (base.sameArch) {
        if (moved > 0) {
          findings.push({
            kind: "base-drift",
            key,
            recorded: record.measured_brotli,
            live: actual.brotli,
          });
        }
      } else if (record.cross_arch_allowance_brotli === undefined) {
        // Without an allowance the line has no bound off the recording's architecture, which
        // must fail rather than pass: a missing number is how this would be switched off.
        findings.push({ kind: "no-allowance", key });
      } else if (moved > record.cross_arch_allowance_brotli) {
        findings.push({
          kind: "cross-arch-drift",
          key,
          recorded: record.measured_brotli,
          live: actual.brotli,
          allowance: record.cross_arch_allowance_brotli,
        });
      }
      continue;
    }

    if (record.measured_sha256_normalised !== actual.sha256Normalised) {
      const moved = Math.abs(actual.brotli - record.measured_brotli);
      if (record.measured_brotli > 0 && moved / record.measured_brotli > tolerance) {
        findings.push({
          kind: "drift",
          key,
          recorded: record.measured_brotli,
          live: actual.brotli,
          tolerance,
        });
      }
      continue;
    }

    if (record.measured_sha256 !== actual.sha256) {
      // Only the generated engine hashes differ. The artifact's own content is unchanged,
      // so its size may move a little -- two 16-hex tokens do not compress identically --
      // but only a little. A tight absolute bound rather than the percentage: this is not
      // drift, it is substitution noise, and 2% of the page would be 433 bytes, which a
      // real CSS regression could hide under.
      const moved = Math.abs(actual.brotli - record.measured_brotli);
      if (moved > HASH_COUPLING_BYTES) {
        findings.push({
          kind: "hash-coupled-drift",
          key,
          recorded: record.measured_brotli,
          live: actual.brotli,
          bound: HASH_COUPLING_BYTES,
        });
      }
      continue;
    }

    // Identical bytes. Any difference in the recorded size is a false record, whatever its
    // size -- there is no tolerance to apply, because nothing changed.
    if (record.measured_raw !== actual.raw) {
      findings.push({
        kind: "false-record",
        key,
        field: "raw",
        recorded: record.measured_raw,
        live: actual.raw,
      });
    }
    if (record.measured_brotli !== actual.brotli) {
      findings.push({
        kind: "false-record",
        key,
        field: "brotli",
        recorded: record.measured_brotli,
        live: actual.brotli,
      });
    }
  }

  return findings;
}

/** Which of the three comparisons an artifact was subject to. */
export type Case = "exact" | "hash-coupled" | "changed" | "no-digest" | "absent";

/**
 * Classify each recorded artifact, so a run can say what it examined.
 *
 * A green `driftFindings` says nothing about WHICH rule each artifact took, and the three
 * are not equally strong: "exact" is the real check, "hash-coupled" is a 64-byte bound, and
 * "changed" is only a percentage. An artifact that quietly moved from the first to the
 * third would weaken the check with nothing to show for it, and a passing run would look
 * identical. `CLAUDE.md`: every check reports what it examined.
 */
export function classify({
  recorded,
  live,
}: Pick<Inputs, "recorded" | "live">): Record<string, Case> {
  const cases: Record<string, Case> = {};
  for (const [key, record] of Object.entries(recorded)) {
    const actual = live[key];
    if (actual === undefined) {
      cases[key] = "absent";
    } else if (!record.measured_sha256 || !record.measured_sha256_normalised) {
      cases[key] = "no-digest";
    } else if (record.measured_sha256_normalised !== actual.sha256Normalised) {
      cases[key] = "changed";
    } else if (record.measured_sha256 !== actual.sha256) {
      cases[key] = "hash-coupled";
    } else {
      cases[key] = "exact";
    }
  }
  return cases;
}

/** A finding, as a line someone can act on. */
export function explain(finding: Finding): string {
  switch (finding.kind) {
    case "no-digest":
      return `${finding.key}: no measured_sha256, so nothing checks that its recorded sizes describe the build. Re-record.`;
    case "false-record":
      return (
        `${finding.key}: the bytes are IDENTICAL to the recording, but measured_${finding.field} ` +
        `says ${finding.recorded} and the build is ${finding.live}. That is a false record, ` +
        `not drift — re-record rather than adjusting the number by hand.`
      );
    case "undeclared-rebuild":
      return (
        `${finding.key}: its bytes differ from the recording, and it is not on the ` +
        `not_byte_reproducible list. Either something changed that should be explained in ` +
        `a commit, or it belongs on that list with a reason.`
      );
    case "hash-coupled-drift":
      return (
        `${finding.key}: only the generated engine hashes differ from the recording, but ` +
        `its size moved from ${finding.recorded} to ${finding.live} brotli — more than the ` +
        `${finding.bound}-byte bound that substitution can explain. Something else changed too.`
      );
    case "base-drift":
      return (
        `${finding.key}: a base line (every visitor loads it) moved from ${finding.recorded} to ` +
        `${finding.live} brotli on the recording's architecture, and base lines have no drift ` +
        `tolerance (DECISIONS rule 18). Re-record it, and name the approval in the PR body.`
      );
    case "no-allowance":
      return (
        `${finding.key}: a base line with no cross_arch_allowance_brotli, so nothing bounds it ` +
        `off the recording's architecture. Measure one from saved runs (rule 18).`
      );
    case "cross-arch-drift":
      return (
        `${finding.key}: moved from the recorded ${finding.recorded} to ${finding.live} brotli ` +
        `on an architecture other than the recording's, past its ${finding.allowance}-byte ` +
        `cross-architecture allowance. STOP, NOT A RE-RECORD (DECISIONS rule 18): the ` +
        `allowance is measured, so this is a change the recording architecture did not see.`
      );
    case "drift":
      return (
        `${finding.key}: declared not byte-reproducible, but its size moved from ` +
        `${finding.recorded} to ${finding.live} brotli, past the ` +
        `${(finding.tolerance * 100).toFixed(0)}% tolerance. That is measured from the ` +
        `recording, not from the last build, so it can be the sum of changes that each ` +
        `passed. Re-record and say what grew.`
      );
  }
}

/** The machine architecture as the budget file spells it. */
export function archName(nodeArch: string): string {
  return nodeArch === "arm64" ? "aarch64" : nodeArch === "x64" ? "x86_64" : nodeArch;
}

/**
 * A payload total against the build (DECISIONS rule 18, 2026-10-18 amendment).
 *
 * On the recording's architecture the live total must equal the recorded one exactly -- every
 * base line is at 0%, so the sum is too. Off it, the total may move at most the summed
 * allowances of the lines in it. `null` when it holds; otherwise the line to print.
 */
export function totalFinding({
  name,
  recorded,
  live,
  sameArch,
  allowances,
}: {
  name: string;
  recorded: number;
  live: number;
  sameArch: boolean;
  allowances: number[];
}): string | null {
  const moved = Math.abs(live - recorded);
  if (sameArch) {
    return moved === 0
      ? null
      : `${name}: the build is ${live} brotli and the recording ${recorded}, on the recording's ` +
          `architecture, where a total equals the exact sum of its measured parts. Re-record.`;
  }
  const bound = allowances.reduce((n, a) => n + a, 0);
  return moved <= bound
    ? null
    : `${name}: the build is ${live} brotli and the recording ${recorded}, ${moved} apart, past ` +
        `the ${bound} bytes its lines' cross-architecture allowances sum to. STOP, NOT A RE-RECORD.`;
}
