// A deliberately small TOML reader, shared by everything in this repository that reads a
// hand-maintained manifest.
//
// IT LIVES HERE BECAUSE THERE MUST BE ONE OF IT. It began inside
// tools/generate-credits.mjs, reading engines/licenses.toml. `apps/web/fonts.toml` is the
// second manifest of this kind, and copying the parser to read it would mean two readers
// that agree until the day they do not -- the same argument engines/licences/README.md
// makes about a committed licence copy, and the same argument ADR 0016 makes about two
// readers of expectations.json.
//
// Node has no TOML parser and this repository will not take an npm dependency for one:
// every dependency is a decision (CLAUDE.md), and pulling a third-party parser in to check
// licences would be a poor trade. The Python half of the engine gate
// (tools/check-engine-licences.py) uses tomllib on the same file, and the two readers must
// agree about what a manifest contains or the page and the gate are checking different
// documents. apps/web/src/credits.test.ts asserts that agreement directly.

/**
 * Parse the TOML subset this repository's manifests use.
 *
 * `source` names the file in every error message, so a failure says which manifest it
 * was reading.
 *
 * Supports: `[table]`, `[[array-of-tables]]`, `key = "basic string"`, `key = true|false`,
 * `key = ["a", "b"]`, and `key = """multi-line"""` with trailing-backslash line joining.
 * Anything else throws rather than being silently skipped -- a licence manifest is the
 * wrong place for a parser that guesses.
 */
export function parseToml(text, source) {
  if (typeof source !== "string" || source === "") {
    // NOT optional, and not defaulted. The parameter exists so a failure says which
    // manifest it was reading; a default would quietly restore the problem for the one
    // caller who forgets, at exactly the moment the message matters.
    throw new Error("parseToml: `source` must name the file being parsed, for error messages");
  }

  const root = {};
  const lines = text.split("\n");
  let current = root;
  let i = 0;

  const setScalar = (target, key, value) => {
    target[key] = value;
  };

  while (i < lines.length) {
    const line = lines[i];
    i += 1;
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#")) continue;

    // NAMES THAT REACH `Object.prototype`. Found by security review of M1 PR A, with a
    // working repro: `[__proto__]` followed by `polluted = "yes"` made `({}).polluted`
    // return "yes" for the rest of the process. `root["__proto__"] ??= {}` is a no-op
    // because `Object.prototype` is not nullish, so `current` BECAME `Object.prototype`
    // and every key after it was written onto the global object. `[[__proto__]]` instead
    // threw `root[key].push is not a function` -- an unhandled crash naming nothing
    // useful -- and `__proto__ = "x"` as an ordinary key was SILENTLY DROPPED, which is
    // precisely the "skipped rather than thrown" outcome the docstring promises cannot
    // happen.
    //
    // It needs commit access to reach, so it is not a live hole. It is fixed rather than
    // shrugged at for two reasons: `generate-credits.mjs` runs as `prebuild` on every CI
    // build, so the polluted process would be the one writing a published page; and the
    // module header's load-bearing claim that this reader and `tomllib` "must agree about
    // what a manifest contains" was FALSE for this input class -- tomllib sees an ordinary
    // table named `__proto__`, this reader saw an empty document and mutated the global
    // object. `credits.test.ts` compares component rosters, so it would not have noticed.
    //
    // Rejecting explicitly rather than switching to `Object.create(null)`: the latter also
    // works, but it makes the hazard invisible at the point it applies, and this parser's
    // contract is that it throws rather than guesses.
    const unsafeName = /^\[?\[?\s*(__proto__|constructor|prototype)\b/.exec(trimmed);
    if (unsafeName !== null) {
      throw new Error(
        `${source}: reserved name on line ${i} is not supported by this reader: ` +
          `${unsafeName[1]}. It would be written onto Object.prototype rather than into ` +
          `the parsed document.`,
      );
    }

    // A dotted table header (`[component.extra]`) nests under the preceding table in real
    // TOML; this reader would flatten it to a root key called "component.extra" and lose the
    // data silently. The docstring promises this parser throws rather than guesses, so it
    // throws. Same for dotted keys below.
    if (/^\[\[?[A-Za-z0-9_-]+\./.test(trimmed)) {
      throw new Error(
        `${source}: dotted table header on line ${i} is not supported by this reader: ` +
          `${trimmed}. Use a flat [[component]] entry, or teach parseToml to nest.`,
      );
    }

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
    if (!kv) throw new Error(`${source}: cannot parse line ${i}: ${trimmed}`);
    if (kv[1].includes(".")) {
      throw new Error(
        `${source}: dotted key on line ${i} is not supported by this reader: ${kv[1]}. ` +
          `It would become a flat key of that literal name rather than a nested table.`,
      );
    }
    const key = kv[1];
    let rest = kv[2];

    // Multi-line basic string. Used for long `note` / `notes` fields.
    if (rest.startsWith('"""')) {
      let body = rest.slice(3);
      while (!body.includes('"""')) {
        if (i >= lines.length) throw new Error(`${source}: unterminated """ for ${key}`);
        body += "\n" + lines[i];
        i += 1;
      }
      body = body.slice(0, body.indexOf('"""'));
      // TOML trims a newline IMMEDIATELY after the opening delimiter. Without this a
      // `notice_required` rewritten as a `"""` block would render with a blank first line.
      body = body.replace(/^\r?\n/, "");
      // A trailing backslash joins the line to the next, per TOML's line-ending backslash.
      // Applied before `unescape` so the join is not mistaken for an escape sequence.
      setScalar(current, key, unescape(body.replace(/\\\n\s*/g, "")));
      continue;
    }

    if (rest.startsWith("[")) {
      // Inline array, possibly spanning lines.
      while ((rest.match(/\[/g) || []).length > (rest.match(/\]/g) || []).length) {
        if (i >= lines.length) throw new Error(`${source}: unterminated [ for ${key}`);
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
      if (!m) throw new Error(`${source}: unterminated string for ${key}`);
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

    throw new Error(`${source}: unsupported value for ${key}: ${rest}`);
  }

  return root;
}

function unescape(s) {
  return s.replace(/\\(["\\nrt])/g, (_, c) =>
    c === "n" ? "\n" : c === "r" ? "\r" : c === "t" ? "\t" : c,
  );
}
