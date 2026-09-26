// Refuse to go on with a build artifact that was built from another tree (#149).
//
// The node half of `tools/build-stamp.py check`, for the two node tools that read the wasm
// artifacts: `stage-web-engines.mjs`, which copies them into every web build, and
// `report-size-budget.mjs`, which measures what that build shipped.
//
// THE CALLER PASSES THE GUARD AS ONE LITERAL STRING -- `"tools/build-stamp.py check pkg ..."` --
// and that is not style. `tools/ci-local.py` finds which jobs read which artifact by looking
// for that literal in what each job runs, so it can refuse a sweep before its first job rather
// than in the middle of it. Assembled from parts, the guard still works here and the sweep's
// preflight no longer sees it.

import { spawnSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..");

/** Exit the process, after the stamper's own report, unless every named artifact is current. */
export function requireCurrentBuild(guard, who) {
  const [script, verb, ...artifacts] = guard.split(" ");
  if (script !== "tools/build-stamp.py" || verb !== "check" || artifacts.length === 0) {
    console.error(`${who}: malformed build-stamp guard ${JSON.stringify(guard)}`);
    process.exit(1);
  }
  // The stamper's report goes to STDERR, its "current" lines included: `deploy.yml` sends
  // `report-size-budget.mjs`'s stdout into the step summary, and that is a size table.
  const done = spawnSync("python3", ["-B", join(repo, script), verb, ...artifacts], {
    stdio: ["ignore", process.stderr, process.stderr],
  });
  if (done.status !== 0) {
    console.error(
      `${who}: refusing -- ${done.error ? `could not run the stamper (${done.error.code})` : "see above"}.`,
    );
    process.exit(1);
  }
}
