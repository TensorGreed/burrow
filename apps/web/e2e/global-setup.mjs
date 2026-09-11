// Build the site with the harness route, then start the two logging servers.
//
// This replaces Playwright's `webServer` block, which could only run one command and could not
// hand the tests a request log. See `e2e/server.mjs` for why the log has to come from the
// server rather than from `page.on("request")`.
//
// The build runs here rather than in `webServer` for the same reason: `globalTeardown` needs
// the server handles, and Playwright's managed server gives none.

import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { startServers } from "./server.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const webApp = resolve(here, "..");
const repo = resolve(webApp, "../..");

/** Where the canary fixture lands. Read by `e2e/console-silence.spec.ts`. */
export const CANARY_PATH = join(webApp, "test-results", "canary.json");

/**
 * Generate the console-silence canary, once for the whole run.
 *
 * The fixture comes from the SAME Rust generator the native leak test uses
 * (`core/burrow-engines/testsupport/minimal_pdf.rs`), because its value is entirely in where
 * the canary sits — a name object, a string, a stream's contents, a dictionary key, and a
 * broken token immediately beside it. A TypeScript reimplementation would be a third copy of
 * that and the first to drift.
 *
 * Done HERE rather than in the spec for two reasons: it runs once instead of once per browser,
 * and a missing toolchain fails with a sentence rather than with `spawnSync cargo ENOENT`
 * three tests deep.
 */
function generateCanary() {
  const mark = `BURROW-CANARY-${randomBytes(8).toString("hex")}`;
  let bytes;
  try {
    bytes = execFileSync(
      "cargo",
      ["run", "-q", "-p", "burrow-engines", "--example", "make-canary-pdf", "--", mark],
      { cwd: repo, maxBuffer: 8 * 1024 * 1024 },
    );
  } catch (error) {
    throw new Error(
      "could not generate the console-silence canary fixture. It comes from " +
        "`cargo run -p burrow-engines --example make-canary-pdf`, so a Rust toolchain has to " +
        "be on PATH to run the e2e suite. Original error: " +
        (error instanceof Error ? error.message : String(error)),
    );
  }
  mkdirSync(dirname(CANARY_PATH), { recursive: true });
  writeFileSync(CANARY_PATH, JSON.stringify({ mark, bytes: Array.from(bytes) }));
}

export default async function globalSetup() {
  const port = Number(process.env.BURROW_TEST_PORT ?? 4321);
  const foreignPort = Number(process.env.BURROW_FOREIGN_PORT ?? 4322);

  if (!process.env.BURROW_SKIP_BUILD) {
    // `BURROW_HARNESS=1` is what puts `/harness` into the build. `prebuild` stages the engines
    // and regenerates the CSP, which is origin-bound — so the port above and the origin the
    // policy names must agree, or every engine fetch is refused.
    execFileSync("pnpm", ["run", "build:harness"], {
      cwd: webApp,
      stdio: "inherit",
      env: { ...process.env, BURROW_SITE: `http://localhost:${port}` },
    });
  }

  generateCanary();

  const servers = await startServers({ port, foreignPort, root: join(webApp, "dist") });

  // Teardown is the RETURNED CLOSURE, which is how Playwright hands `globalSetup` a cleanup
  // hook without a separate `globalTeardown` file. An earlier version also stashed the handles
  // on the config object with a comment calling that "the only channel Playwright offers" —
  // which was both unused and untrue, since this is the channel.
  return async () => {
    servers.site.close();
    servers.foreign.close();
  };
}
