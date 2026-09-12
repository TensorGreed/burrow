#!/usr/bin/env node
// Generate the website credits page's data from engines/licenses.toml.
//
// WHY THIS IS GENERATED, AND WHY IT IS A LICENCE OBLIGATION RATHER THAN A NICETY
//
// ADR 0008 and ADR 0010 admit three licences that impose AFFIRMATIVE notice obligations
// binding executable-only distribution -- which is exactly what a wasm bundle is:
//
//   * FreeType, FTL section 2      -- "based in part of the work of the FreeType Team"
//   * Independent JPEG Group, (2)  -- "based in part on the work of the Independent JPEG Group"
//   * HarfBuzz, MIT-Modern-Variant -- the copyright notice AND both disclaimer paragraphs
//
// All three say *documentation accompanying the distribution*. A file in the git repository
// does not satisfy that for a website: the user has to be able to reach it. ADR 0008 names
// four surfaces and this is the second of them; the two app screens land at M3 and M4.
//
// Generated rather than written by hand for one reason: a hand-written page passes review
// once and then rots. This one is derived from the SAME manifest tools/check-engine-licences.py
// gates in CI, so the page cannot drift from what the licence check believes we ship. Add a
// component to the manifest without the page following and the build-output test fails.
//
// FTL SECTION 3: the FreeType name must NOT be used to promote burrow. Nothing does today.
// If a "powered by" badge is ever proposed, that is the clause it violates.
//
// INPUTS ARE COMMITTED FILES ONLY. Every `license_text` path resolves inside the repository,
// never into engines/vendor/, which is gitignored -- see engines/licences/README.md. That is
// what lets `pnpm build` produce a complete credits page from a clean checkout, and what lets
// CI's web job (which stages only the wasm vendor prefix) build it at all.

import { readFile, mkdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..");
const generatedDir = join(repo, "apps", "web", "src", "generated");

/**
 * A deliberately small TOML reader for the subset engines/licenses.toml uses.
 *
 * Node has no TOML parser and this repository will not take an npm dependency for one --
 * every dependency is a decision (CLAUDE.md), and a licence-manifest reader pulling in a
 * third-party parser to check licences would be a poor trade. The Python half of the gate
 * (tools/check-engine-licences.py) uses tomllib on the same file, and the two readers must
 * agree about what the manifest contains, or the page and the gate are checking different
 * documents. `credits.test.ts` asserts that agreement directly: it re-reads the manifest with
 * tomllib and compares the component roster, name for name, against the generated page.
 *
 * Supports: `[table]`, `[[array-of-tables]]`, `key = "basic string"`, `key = true|false`,
 * `key = ["a", "b"]`, and `key = """multi-line"""` with trailing-backslash line joining.
 * Anything else throws rather than being silently skipped -- a licence manifest is the
 * wrong place for a parser that guesses.
 */
function parseToml(text) {
  const root = {};
  const lines = text.split("\n");
  let current = root;
  let i = 0;

  const setScalar = (target, key, value) => {
    target[key] = value;
  };

  while (i < lines.length) {
    let line = lines[i];
    i += 1;
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#")) continue;

    const arrayTable = /^\[\[([A-Za-z0-9_.-]+)\]\]$/.exec(trimmed);
    if (arrayTable) {
      const key = arrayTable[1];
      root[key] ??= [];
      current = {};
      root[key].push(current);
      continue;
    }

    const table = /^\[([A-Za-z0-9_.-]+)\]$/.exec(trimmed);
    if (table) {
      root[table[1]] ??= {};
      current = root[table[1]];
      continue;
    }

    const kv = /^([A-Za-z0-9_.-]+)\s*=\s*(.*)$/.exec(trimmed);
    if (!kv) throw new Error(`licenses.toml: cannot parse line ${i}: ${trimmed}`);
    const key = kv[1];
    let rest = kv[2];

    // Multi-line basic string. Used for long `note` / `notes` fields.
    if (rest.startsWith('"""')) {
      let body = rest.slice(3);
      while (!body.includes('"""')) {
        if (i >= lines.length) throw new Error(`licenses.toml: unterminated """ for ${key}`);
        body += "\n" + lines[i];
        i += 1;
      }
      body = body.slice(0, body.indexOf('"""'));
      // A trailing backslash joins the line to the next, per TOML's line-ending backslash.
      setScalar(current, key, body.replace(/\\\n\s*/g, ""));
      continue;
    }

    if (rest.startsWith("[")) {
      // Inline array, possibly spanning lines.
      while ((rest.match(/\[/g) || []).length > (rest.match(/\]/g) || []).length) {
        if (i >= lines.length) throw new Error(`licenses.toml: unterminated [ for ${key}`);
        rest += " " + lines[i].trim();
        i += 1;
      }
      const inner = rest.slice(rest.indexOf("[") + 1, rest.lastIndexOf("]"));
      const items = [...inner.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => unescape(m[1]));
      setScalar(current, key, items);
      continue;
    }

    if (rest.startsWith('"')) {
      const m = /^"((?:[^"\\]|\\.)*)"/.exec(rest);
      if (!m) throw new Error(`licenses.toml: unterminated string for ${key}`);
      setScalar(current, key, unescape(m[1]));
      continue;
    }

    if (rest === "true" || rest === "false") {
      setScalar(current, key, rest === "true");
      continue;
    }

    const num = /^-?\d+$/.exec(rest);
    if (num) {
      setScalar(current, key, Number(rest));
      continue;
    }

    throw new Error(`licenses.toml: unsupported value for ${key}: ${rest}`);
  }

  return root;
}

function unescape(s) {
  return s.replace(/\\(["\\nrt])/g, (_, c) =>
    c === "n" ? "\n" : c === "r" ? "\r" : c === "t" ? "\t" : c,
  );
}

async function main() {
  const manifestPath = join(repo, "engines", "licenses.toml");
  const manifest = parseToml(await readFile(manifestPath, "utf8"));

  const components = manifest.component ?? [];
  if (components.length === 0) {
    throw new Error("engines/licenses.toml declares no components");
  }

  // Every component ships in the notice, linked or not: `linked = false` means "in the
  // package but not found in the binary", and we distribute the package's contents either
  // way. What `linked` changes is whether a committed licence TEXT is mandatory.
  const entries = [];
  for (const comp of components) {
    const name = comp.name;
    if (!name) throw new Error("engines/licenses.toml: a component has no name");

    let text = null;
    if (comp.license_text) {
      text = await readFile(join(repo, comp.license_text), "utf8");
      if (text.trim() === "") {
        throw new Error(`${name}: license_text ${comp.license_text} is empty`);
      }
    } else if (comp.linked) {
      // check-engine-licences.py is the gate for this; failing here too means a broken
      // manifest cannot produce a quietly incomplete page.
      throw new Error(`${name}: linked = true but no license_text. See engines/licences/README.md`);
    }

    entries.push({
      name,
      version: comp.version ?? null,
      license: comp.license,
      linked: comp.linked === true,
      testOnly: comp.test_only === true,
      artifacts: comp.artifacts ?? [],
      noticeRequired: comp.notice_required ?? null,
      licenseTextPath: comp.license_text ?? null,
      licenseText: text,
    });
  }

  const notices = entries.filter((e) => e.noticeRequired !== null);
  if (notices.length === 0) {
    // Three obligations exist today. Zero means the manifest lost its `notice_required`
    // fields, which would silently produce a page that discharges nothing.
    throw new Error("engines/licenses.toml declares no notice_required obligations");
  }

  const payload = {
    generatedBy: "tools/generate-credits.mjs from engines/licenses.toml",
    policyAdr: manifest.meta?.policy_adr ?? "docs/adr/0008-widened-licence-allowlist.md",
    lastAudited: manifest.meta?.last_audited ?? null,
    components: entries,
  };

  const out = `// GENERATED by tools/generate-credits.mjs. Do not edit.
//
// Derived from engines/licenses.toml, the same manifest tools/check-engine-licences.py
// gates in CI, so the credits page cannot drift from the licence check. See ADR 0008.
export const CREDITS = ${JSON.stringify(payload, null, 2)};
`;

  await mkdir(generatedDir, { recursive: true });
  await writeFile(join(generatedDir, "credits.js"), out);

  console.log(
    `credits: ${entries.length} components (${entries.filter((e) => e.linked).length} linked), ` +
      `${notices.length} notice obligations, ` +
      `${entries.filter((e) => e.licenseText).length} licence texts`,
  );
}

await main();
