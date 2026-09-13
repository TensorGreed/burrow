// `tools/toml.mjs` — the shared manifest reader.
//
// WHY THIS FILE EXISTS, AND WHY IT IS HERE
//
// The parser began inside `tools/generate-credits.mjs`, where its behaviour was covered
// incidentally: `credits.test.ts` compares the generated page's component roster against
// what `tomllib` reads from the same manifest, so a parser bug that changed the roster
// showed up. M1 PR A moved it to `tools/toml.mjs` and gave it a second consumer
// (`fonts.test.ts`). That is exactly the moment incidental coverage stops being coverage:
// the roster comparison says nothing about a manifest shape only the font file uses.
//
// It lives under `apps/web/src/` rather than `tools/` because vitest is the only test
// runner in this repository that can import an ES module, and it is rooted here. The
// alternative — a Python-side test, as the engine tools have — would be testing a
// different reader.
//
// EVERY VALUE FORM AND EVERY THROW PATH GETS A CASE. The docstring promises this parser
// "throws rather than being silently skipped", and a promise like that is only worth what
// its negative cases are worth: a parser that quietly returns `{}` makes every rule built
// on it vacuously true, which is the failure `tokens-check.ts`'s header catalogues.

import { describe, expect, it } from "vitest";
import { parseToml } from "../../../tools/toml.mjs";

const parse = (text: string) => parseToml(text, "probe.toml") as Record<string, unknown>;

describe("the value forms it supports", () => {
  it("reads a basic string", () => {
    expect(parse('[t]\na = "b"\n')).toEqual({ t: { a: "b" } });
  });

  it("reads booleans and integers as their own types, not as strings", () => {
    expect(parse("[t]\nyes = true\nno = false\nn = 42\nneg = -7\n")).toEqual({
      t: { yes: true, no: false, n: 42, neg: -7 },
    });
  });

  it("reads an inline array, including one spanning lines", () => {
    expect(parse('[t]\na = ["x", "y"]\n')).toEqual({ t: { a: ["x", "y"] } });
    expect(parse('[t]\na = [\n  "x",\n  "y",\n]\n')).toEqual({ t: { a: ["x", "y"] } });
  });

  it("reads a multi-line string, trimming the newline after the opening delimiter", () => {
    // TOML trims a newline immediately after `"""`. Without that, every `"""` block in
    // engines/licenses.toml would render with a blank first line on the credits page.
    expect(parse('[t]\na = """\nline one\nline two"""\n')).toEqual({
      t: { a: "line one\nline two" },
    });
  });

  it("joins a multi-line string's trailing backslash to the next line", () => {
    expect(parse('[t]\na = """\nend of \\\n  line"""\n')).toEqual({ t: { a: "end of line" } });
  });

  it("unescapes the sequences it claims to", () => {
    expect(parse('[t]\na = "q\\"q\\\\z\\nn\\tt"\n')).toEqual({ t: { a: 'q"q\\z\nn\tt' } });
  });

  it("builds an array of tables, in order", () => {
    expect(parse('[[c]]\nid = "one"\n\n[[c]]\nid = "two"\n')).toEqual({
      c: [{ id: "one" }, { id: "two" }],
    });
  });

  it("ignores comments and blank lines", () => {
    expect(parse('# a comment\n\n[t]\n# another\na = "b"\n')).toEqual({ t: { a: "b" } });
  });
});

describe("what it refuses", () => {
  // Each of these is a documented promise. Without a case per path, "throws rather than
  // guessing" is a sentence in a docstring rather than a property of the code.

  it("refuses a dotted table header rather than flattening it", () => {
    expect(() => parse('[a.b]\nx = "y"\n')).toThrow(/dotted table header/);
  });

  it("refuses a dotted key rather than making a flat key of that literal name", () => {
    expect(() => parse('[t]\na.b = "y"\n')).toThrow(/dotted key/);
  });

  it("refuses an unterminated multi-line string", () => {
    expect(() => parse('[t]\na = """\nnever closed\n')).toThrow(/unterminated """/);
  });

  it("refuses an unterminated array", () => {
    expect(() => parse('[t]\na = ["x",\n')).toThrow(/unterminated \[/);
  });

  it("refuses an unterminated string", () => {
    expect(() => parse('[t]\na = "x\n')).toThrow(/unterminated string/);
  });

  it("refuses a value form it does not understand", () => {
    // A float, for instance. Reading it as something else would be guessing.
    expect(() => parse("[t]\na = 1.5\n")).toThrow(/unsupported value/);
  });

  it("refuses a line that is not a key/value pair at all", () => {
    expect(() => parse("[t]\nnonsense\n")).toThrow(/cannot parse line/);
  });

  it("names the file it was reading in every message", () => {
    // The whole reason `source` is a required parameter. A build that fails while reading
    // one of two manifests should say which.
    expect(() => parse('[a.b]\nx = "y"\n')).toThrow(/^probe\.toml:/);
  });

  it("requires a source, rather than defaulting to a name that says nothing", () => {
    // No `@ts-expect-error` here: `tools/toml.mjs` is plain JS with JSDoc and no `.d.ts`,
    // so TypeScript does not see the arity at all. Which is the point — nothing but this
    // test stops a caller from omitting the argument.
    expect(() => (parseToml as (t: string) => unknown)('a = "b"\n')).toThrow(/must name the file/);
  });
});

describe("names that would reach Object.prototype", () => {
  // FOUND BY SECURITY REVIEW OF M1 PR A, WITH A WORKING REPRO. `[__proto__]` followed by
  // `polluted = "yes"` made `({}).polluted` return "yes" for the rest of the process,
  // because `root["__proto__"] ??= {}` is a no-op — Object.prototype is not nullish — so
  // `current` became Object.prototype itself.
  //
  // It needs commit access to reach, so it was never a live hole. It is tested because
  // `generate-credits.mjs` runs as `prebuild` on every CI build, so the polluted process
  // would have been the one writing a published page.

  it.each([
    ["table header", '[__proto__]\npolluted = "yes"\n'],
    ["array-of-tables header", '[[__proto__]]\nx = "y"\n'],
    ["bare key", '__proto__ = "x"\n'],
    ["constructor as a table", '[constructor]\na = "b"\n'],
    ["prototype as a key", '[t]\nprototype = "x"\n'],
  ])("refuses %s", (_what, text) => {
    expect(() => parse(text)).toThrow(/reserved name/);
  });

  it("leaves Object.prototype untouched after all of the above", () => {
    // The assertion that would have failed before the fix. Written against the object
    // itself rather than the parse result, because the parse result looked innocent.
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
    expect(Object.prototype).not.toHaveProperty("polluted");
  });

  it("still accepts an ordinary name that merely contains one of those words", () => {
    // The near-miss. A guard that rejected `reconstructor` or `proto_notes` would be one
    // that matches too much, which is the other half of the rule in CLAUDE.md's definition
    // of done — and a manifest key is a plausible place for such a name.
    expect(parse('[t]\nreconstructor = "x"\nproto_notes = "y"\n')).toEqual({
      t: { reconstructor: "x", proto_notes: "y" },
    });
  });
});

describe("the manifests it actually reads", () => {
  it("round-trips the shapes both real manifests use", () => {
    // A smoke case in the shape of the two committed files, so a change that breaks them
    // fails here with a small diff rather than in a generator with a large one.
    const parsed = parse(
      '[meta]\nadr = "docs/adr/0008.md"\n\n' +
        '[[component]]\nname = "zlib"\nlinked = true\nartifacts = ["a", "b"]\n' +
        'note = """\nwhy it is here"""\n\n' +
        '[[font]]\nid = "x"\nbytes = 18548\n',
    );
    expect(parsed).toEqual({
      meta: { adr: "docs/adr/0008.md" },
      component: [{ name: "zlib", linked: true, artifacts: ["a", "b"], note: "why it is here" }],
      font: [{ id: "x", bytes: 18548 }],
    });
  });
});
