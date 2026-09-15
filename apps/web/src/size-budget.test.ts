// The first-load size budget, asserted against a real production build.
//
// ROADMAP item 11: "record the module size and fail on an unexplained regression". Two words
// there do the work.
//
// **Module size is not the thing to budget.** What a user pays is the sum of everything
// fetched before their first operation can run: the page shell, the worker bundle, all three
// .wasm modules, and the CSP control file. Budgeting them individually lets three files each
// grow 4% -- a 4% regression -- while every per-file budget passes. So the total is the
// binding gate here and the per-artifact lines exist to say where it went. There is a test
// below that plants exactly that distributed regression and asserts the total catches it.
//
// **"Unexplained" is what the budget file is for.** `size-budget.json` records the measured
// value beside the budget, so raising one means editing a number next to the measurement it
// came from and saying why in the commit. A budget that is raised without anyone knowing what
// grew has stopped being a budget.
//
// Brotli, because that is what a host serves. Raw bytes are recorded and reported -- they are
// what the browser compiles, and what spike 0001 measured -- but they are not gated: a change
// that made the module compress better while getting larger on disk is not a regression a
// user experiences.
//
// The build comes from `vitest.global-setup.ts`, with no `BURROW_HARNESS` set. Measuring a
// harness build would count `host/` and the harness route, which no deploy ships.

import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  byBudgetKey,
  digestsByBudgetKey,
  heaviestFirstLoad,
  normaliseEngineHashes,
} from "../../../tools/first-load.mjs";
import {
  HASH_COUPLING_BYTES,
  type Live,
  classify,
  driftFindings,
  explain,
} from "./size-budget-drift.js";
import { PRODUCTION_DIR } from "./build-output.js";

const webApp = resolve(dirname(fileURLToPath(import.meta.url)), "..");

interface Line {
  measured_route?: string;
  measured_raw: number;
  measured_brotli: number;
  measured_sha256?: string;
  measured_sha256_normalised?: string;
  budget_brotli: number;
  why?: string;
}

const budget: {
  headroom: { per_artifact: number; page: number; total: number; why: string[] };
  artifacts: Record<string, Line>;
  total: Line;
  not_byte_reproducible: Record<string, string>;
  drift_tolerance: number;
} = JSON.parse(readFileSync(join(webApp, "size-budget.json"), "utf8"));

// THE HEAVIEST LANDING ROUTE, not `index.html`. People arrive from a search for "merge pdf"
// and land on `/merge-pdf`, which carries an island bundle the home page does not; budgeting
// the home page would budget the lightest route and leave the heaviest one unwatched. Which
// route won is recorded in the budget file and asserted below, so the substitution can never
// be silent.
const heaviest = heaviestFirstLoad(PRODUCTION_DIR);
const measurement = heaviest.measurement;
const groups = byBudgetKey(measurement);
const digests = digestsByBudgetKey(PRODUCTION_DIR, groups);

const live: Record<string, Live> = Object.fromEntries(
  Object.entries(groups).map(([key, group]) => [
    key,
    {
      raw: group.raw,
      brotli: group.brotli,
      sha256: digests[key].raw,
      sha256Normalised: digests[key].normalised,
    },
  ]),
);

const kb = (n: number) => `${(n / 1024).toFixed(1)} KiB`;

describe("the first-load size budget", () => {
  it("weighs every route in the build, by name", () => {
    // NAMES, NOT A COUNT, and not a comparison against the same reduce that produced the
    // answer. The first version of this asserted that the chosen route was the largest of
    // `considered` — both sides computed by `heaviestFirstLoad`, from the same array, with
    // the same tie-break. It could only fail if six lines of one function disagreed with
    // themselves, and it could NOT fail for the thing that would actually go wrong:
    // `landingPages()` quietly dropping a route, which is how a tool page would end up
    // unbudgeted. Code review caught it; it is the tautology shape the working agreements
    // already record from M1 PR 4a-ii.
    //
    // The expected set is derivable, so it is derived: one route per page source, minus
    // `credits` (deliberately out of scope — see `tools/first-load.mjs`) and minus the
    // harness, which no production build contains.
    const routes = readdirSync(join(webApp, "src", "pages"))
      .filter((name) => name.endsWith(".astro"))
      .map((name) => name.replace(/\.astro$/, ""))
      .filter((name) => name !== "credits")
      .map((name) => (name === "index" ? "index.html" : `${name}/index.html`))
      .sort();

    expect(
      heaviest.considered.map((c) => c.page).sort(),
      "the routes weighed are not the routes this app has pages for",
    ).toEqual(routes);
    expect(routes.length, "a single-route build makes 'the heaviest' mean nothing").toBeGreaterThan(
      1,
    );
  });

  it("puts every script on the bounded line and nothing else on it", () => {
    // THE SPLIT IS THE CHECK, so it is pinned by name rather than left to a comment. `page`
    // keeps the exact digest and `page-js` takes the bound, and that is only sound while the
    // membership is what it claims: a stylesheet that drifted onto the bounded line would be
    // a CSS regression with 10% of room to hide in.
    const scripts = groups["page-js"]?.files ?? [];
    const rest = groups.page?.files ?? [];

    expect(
      scripts.length,
      "no scripts in the payload at all; the split is measuring nothing",
    ).toBeGreaterThan(0);
    for (const path of scripts) {
      expect(path, `${path} is on the scripts line and is not a script`).toMatch(/\.m?js$/);
    }
    for (const path of rest) {
      expect(path, `${path} is a script and must not be on the exact line`).not.toMatch(/\.m?js$/);
    }

    // And the exemption lists are pinned, for the reason `tokens.test.ts` pins its own:
    // moving a line between them is how a check gets quietly weakened, and it should be a
    // diff a reviewer sees rather than a number that changed.
    expect(Object.keys(budget.not_byte_reproducible).sort()).toEqual([
      "engines/burrow-worker.js",
      "engines/burrow_wasm_bg.wasm",
      "engines/qpdf.wasm",
      "page-js",
    ]);
  });

  it("follows what a script imports, by walking the imports itself", () => {
    // AN INDEPENDENT ROUTE TO THE SAME COUNT. `firstLoad` finds these by one regex over the
    // markup and then over each script; this re-derives them by reading the entry script and
    // resolving its specifiers, which is the cross-check the definition of done asks for when
    // a pattern could be blind to a class of names.
    //
    // It exists because that pattern WAS blind: the first version bounded the distance between
    // `import` and the specifier at 64 characters, and a minified island's destructured import
    // list is hundreds. It matched nothing, and the Svelte runtime -- 31 KB, a third of the
    // page's JavaScript -- was a download no budget line could see.
    const scripts = groups["page-js"].files;
    const entry = scripts.find((f) => f.includes("astro_type_script"));
    expect(entry, "no Astro page script in the payload").toBeDefined();

    const source = readFileSync(join(PRODUCTION_DIR, entry as string), "utf8");
    const imported = [...source.matchAll(/\bfrom\s*["']\.\/([^"']+)["']/g)].map((m) => m[1]);
    expect(
      imported.length,
      "the entry script imports nothing, so this cross-check confirms nothing",
    ).toBeGreaterThan(0);

    for (const name of imported) {
      expect(
        scripts,
        `${entry} imports ${name}, which the payload does not include — it is a real download ` +
          `no budget line can see`,
      ).toContain(`_astro/${name}`);
    }
  });

  it("budgets the route the build says is heaviest", () => {
    expect(
      budget.artifacts.page.measured_route,
      `the budget records ${budget.artifacts.page.measured_route ?? "no route"} but the ` +
        `heaviest route in this build is ${heaviest.page}. A route overtaking the recorded ` +
        `one is a finding: re-measure and say so, do not let the line change what it means`,
    ).toBe(heaviest.page);
  });

  it("states each tool page's ceilings once, and that page's prose repeats them", () => {
    // `apps/web/CLAUDE.md`: "It sends the files and reports what the core refuses, so the
    // prose and the code can be caught disagreeing." That was true of the page ceiling --
    // `e2e/merge-pdf.spec.ts` drives a real refusal past it -- and NOT true of the byte
    // ceiling, which appears in the island and in the page's prose with nothing comparing
    // them. Code review found it; this is the comparison.
    //
    // EVERY TOOL PAGE, not just the first. It hard-coded `/merge-pdf` and its island, so
    // `/rotate-pdf` shipped the same two numbers in two more places with nothing comparing
    // them -- and tools three to five would each have added two more. Found by code review on
    // the rotate page. The pairs are DERIVED from the pages that exist: a page added without
    // a line here is still checked, which is the whole point.
    const pagesDir = join(webApp, "src", "pages");
    const toolPages = readdirSync(pagesDir).filter((f) => /^[a-z-]+-pdf\.astro$/.test(f));

    // ONE SET OF CEILINGS, read once. They used to be declared per island, so each page had
    // its own copy to keep in step; they now live in `src/components/tool-host.ts`, which
    // makes this one comparison against N pages rather than N pairs.
    const shared = readFileSync(join(webApp, "src", "components", "tool-host.ts"), "utf8");
    const bytes = /maxInputBytes:\s*(\d+)\s*\*\s*1024\s*\*\s*1024/.exec(shared);
    const pages = /maxPages:\s*([\d_]+)/.exec(shared);
    expect(
      bytes,
      "maxInputBytes is no longer written as N * 1024 * 1024 in tool-host.ts; update this rule",
    ).not.toBeNull();
    expect(
      pages,
      "maxPages is not where this rule looks in tool-host.ts; update it",
    ).not.toBeNull();
    const mb = Number(bytes?.[1]);
    const maxPages = Number(pages?.[1].replace(/_/g, ""));

    let checked = 0;
    for (const pageFile of toolPages) {
      // Whitespace-normalised: the prose is wrapped at 100 columns, so "512 MB" is really
      // "512\n      MB" in the source and a naive search would report a disagreement that is
      // only a line break.
      const prose = readFileSync(join(pagesDir, pageFile), "utf8").replace(/\s+/g, " ");

      expect(prose, `${pageFile} refuses at ${mb} MB and its prose does not say so`).toContain(
        `${mb} MB`,
      );
      expect(
        prose,
        `${pageFile} refuses at ${maxPages} pages and its prose does not say so`,
      ).toContain(maxPages.toLocaleString("en-GB"));
      checked += 1;
    }

    // GATED ON THE COUNT. A resolver that found no pages would pass this whether or not any
    // page disagreed with its island -- "0 of 5" reads exactly like success.
    expect(checked, "no tool page was checked, so this compares nothing").toBe(toolPages.length);
    expect(
      checked,
      "fewer tool pages than expected; a page was added without a route",
    ).toBeGreaterThanOrEqual(2);
  });

  it("stays within the total budget", () => {
    // THE GATE. Everything else in this file is diagnosis.
    expect(
      measurement.total.brotli,
      `first load is ${kb(measurement.total.brotli)} brotli, over the ` +
        `${kb(budget.total.budget_brotli)} budget. Find what grew in the per-artifact ` +
        `failures below; do not raise this number without knowing`,
    ).toBeLessThanOrEqual(budget.total.budget_brotli);
  });

  it("stays within every per-artifact budget", () => {
    const over: string[] = [];
    for (const [key, line] of Object.entries(budget.artifacts)) {
      const actual = groups[key];
      if (!actual) continue; // covered by the coverage test below
      if (actual.brotli > line.budget_brotli) {
        over.push(
          `${key}: ${kb(actual.brotli)} > ${kb(line.budget_brotli)} ` +
            `(was ${kb(line.measured_brotli)} when the budget was set)`,
        );
      }
    }
    expect(over, `over budget:\n  ${over.join("\n  ")}`).toEqual([]);
  });

  it("budgets every artifact the payload actually contains", () => {
    // Without this, a NEW engine artifact -- a third .wasm, a second bundle -- would be
    // counted in the total and budgeted by nothing, and the first person to notice would be
    // whoever raised the total to make CI pass.
    const unbudgeted = Object.keys(groups).filter((key) => !(key in budget.artifacts));
    expect(
      unbudgeted,
      `first load contains artifacts with no budget: ${unbudgeted.join(", ")}. ` +
        `Add them to size-budget.json with a measured value and a reason`,
    ).toEqual([]);
  });

  it("has no budget line for an artifact that no longer ships", () => {
    // The other direction. A stale line is a budget nothing can ever trip, and a reader
    // would take it for coverage.
    const stale = Object.keys(budget.artifacts).filter((key) => !(key in groups));
    expect(stale, `size-budget.json budgets files that do not ship: ${stale.join(", ")}`).toEqual(
      [],
    );
  });

  it("measures a payload that is actually the shipped one", () => {
    // A budget over an empty or truncated measurement passes trivially. Pin the shape: every
    // wasm module, the worker bundle, the control file, and a page.
    const keys = Object.keys(groups);
    for (const required of [
      "engines/qpdf.wasm",
      "engines/burrow_wasm_bg.wasm",
      "engines/burrow-worker.js",
      "engines/control.txt",
      "page",
    ]) {
      expect(keys, `${required} is not in the measured payload`).toContain(required);
    }
    // AND NO PDFIUM. It was 79.7% of this payload and the assertion here was that it stayed
    // above 70% — which was the right shape of check (a payload missing its bulk is a broken
    // measurement) pointed at an engine the web no longer loads. Spike 0004 removed it, so the
    // same idea is now qpdf: it is the bulk, and a measurement where it is not has gone wrong.
    expect(keys, "PDFium is not supposed to ship to the web").not.toContain("engines/pdfium.wasm");
    expect(groups["engines/qpdf.wasm"].brotli / measurement.total.brotli).toBeGreaterThan(0.5);
  });

  it("catches a regression split across three files, which no per-file budget would", () => {
    // The reason the total exists, planted rather than argued: a growth that every per-artifact
    // line waves through and the total still refuses.
    //
    // THE GROWTH IS DERIVED FROM THE TWO HEADROOMS, not hardcoded. It was a literal 4%, which
    // silently encoded the 3%-total / 10%-per-artifact split this file used to have — so when
    // spike 0004 cut the payload by 80.7% and the total moved to 10%, the number stopped
    // expressing the property and the test failed against a budget that was fine. The property
    // is that the total is the TIGHTER gate, and the midpoint between the two headrooms is a
    // growth that demonstrates it whenever that is true. If the two are ever equal there is no
    // such growth, and the assertion below says so rather than a magic number quietly
    // preserving an arrangement nobody restated.
    expect(
      budget.headroom.total,
      "the total must be tighter than the per-artifact lines, or it cannot catch what they miss",
    ).toBeLessThan(budget.headroom.per_artifact);
    const growth = 1 + (budget.headroom.total + budget.headroom.per_artifact) / 2;

    // Computed against the *recorded* measurements rather than the live ones, so this test
    // asserts a property of the budget file and cannot be made vacuous by the build changing.
    const grown = Object.fromEntries(
      Object.entries(budget.artifacts).map(([k, v]) => [k, Math.round(v.measured_brotli * growth)]),
    );

    for (const [key, value] of Object.entries(grown)) {
      expect(
        value,
        `${key} at +${Math.round((growth - 1) * 100)}% would already exceed its own budget, ` +
          `so this test proves nothing`,
      ).toBeLessThanOrEqual(budget.artifacts[key].budget_brotli);
    }

    const total = Object.values(grown).reduce((n, v) => n + v, 0);
    expect(
      total,
      `a +${Math.round((growth - 1) * 100)}% regression spread across every artifact would pass the ` +
        "total budget. " +
        "The total's headroom is too loose to be the gate it claims to be",
    ).toBeGreaterThan(budget.total.budget_brotli);
  });

  it("has per-artifact measurements that add up to the recorded total", () => {
    // The "+4% across three files" test above sums `artifacts[*].measured_brotli` and
    // compares the result to `total.budget_brotli`. That is only meaningful while the two
    // halves of this file describe the same build: a per-artifact number left stale after a
    // re-measure would silently weaken it, and nothing else would notice.
    const summed = Object.values(budget.artifacts).reduce((n, a) => n + a.measured_brotli, 0);
    expect(
      summed,
      "size-budget.json's per-artifact measurements do not sum to its recorded total; " +
        "one half was re-measured and the other was not",
    ).toBe(budget.total.measured_brotli);
  });

  it("records the measurement each budget was set from", () => {
    // `measured_brotli` is what makes a budget auditable: a reviewer can see how much slack a
    // line has without rebuilding. A budget below its own measurement is a typo that would
    // fail on the very build it was taken from.
    for (const [key, line] of Object.entries(budget.artifacts)) {
      expect(line.measured_brotli, `${key} records no measurement`).toBeGreaterThan(0);
      expect(line.budget_brotli, `${key}'s budget is below its own measurement`).toBeGreaterThan(
        line.measured_brotli,
      );
      expect(line.why, `${key} has no stated reason`).toBeTruthy();
    }
    expect(budget.total.budget_brotli).toBeGreaterThan(budget.total.measured_brotli);
  });

  it("has not drifted far from the recorded measurements without anyone noticing", () => {
    // A budget file whose measurements are stale still gates correctly, but it stops being
    // readable: `measured_brotli` no longer tells a reviewer how much slack there is. This
    // fails at half the remaining headroom, which is a request to re-measure rather than a
    // regression -- and it fires before the budget does, so it is a warning with a diff
    // rather than a red build with no explanation.
    const halfway =
      budget.total.measured_brotli +
      (budget.total.budget_brotli - budget.total.measured_brotli) / 2;
    expect(
      measurement.total.brotli,
      `first load is ${kb(measurement.total.brotli)}, more than halfway from the recorded ` +
        `${kb(budget.total.measured_brotli)} to the ${kb(budget.total.budget_brotli)} budget. ` +
        `Re-measure and update size-budget.json, or find out what grew`,
    ).toBeLessThanOrEqual(halfway);
  });
});

describe("the recording describes the build it claims to", () => {
  // THE DRIFT PROBE. The tests above gate live sizes against budgets and the recorded lines
  // against each other; neither compares a recorded measurement with the build. So a
  // recording that drifts CONSISTENTLY satisfies every one of them, which is how
  // `engines/burrow-worker.js` sat 236 brotli bytes light on an unchanged checkout with
  // nothing failing. M1 PR A corrected the number. A corrected number is not a control.

  it("finds no drift against this build", () => {
    const findings = driftFindings({
      recorded: budget.artifacts,
      live,
      notByteReproducible: budget.not_byte_reproducible,
      tolerance: budget.drift_tolerance,
    });
    expect(findings.map(explain)).toEqual([]);
  });

  it("says which comparison each artifact was subject to, and examines every one", () => {
    // THE COUNT AND THE BREAKDOWN ARE THE MEASUREMENT. A green `driftFindings` does not say
    // WHICH rule each artifact took, and the three are not equally strong: "exact" is the
    // real check, "hash-coupled" is a 64-byte bound, "changed" is only a percentage. An
    // artifact sliding from the first to the third weakens the check with nothing to show
    // for it, and the run still passes.
    //
    // It is a breakdown rather than a fixed expectation per artifact because which case an
    // artifact takes legitimately DIFFERS between here and CI -- `page` is exact locally
    // and hash-coupled in CI, because CI rebuilds qpdf.wasm and the page quotes its hash.
    // Pinning the case per artifact would fail in one place or the other for no good
    // reason. What must hold everywhere is that every artifact was compared by something.
    const cases = classify({ recorded: budget.artifacts, live });

    const breakdown = Object.entries(cases)
      .map(([key, kind]) => `${key}=${kind}`)
      .sort()
      .join(" ");

    // PER FILE, FOR THE POOLED LINES, because a pooled digest that disagrees says only that
    // one of six files moved. Learning WHICH cost three round trips to CI, and the answer was
    // not one anybody would have guessed. The group digests are the gate; this is the log
    // line that makes a red one actionable without a push.
    for (const [key, group] of Object.entries(groups)) {
      if (group.files.length < 2) continue;
      const perFile = [...group.files]
        .sort()
        .map((path) => {
          const bytes = normaliseEngineHashes(readFileSync(join(PRODUCTION_DIR, path)), path);
          return `${path}=${createHash("sha256").update(bytes).digest("hex").slice(0, 12)}`;
        })
        .join("\n    ");
      console.log(`  ${key}, file by file:\n    ${perFile}`);
    }
    expect(Object.keys(cases)).toHaveLength(Object.keys(budget.artifacts).length);
    expect(
      Object.values(cases).filter((c) => c === "no-digest" || c === "absent"),
      `every artifact must be compared by one of the three rules: ${breakdown}`,
    ).toEqual([]);

    // Deliberately not silent on success: the breakdown is what a reader needs to tell a
    // strong pass from a weak one, and it is invisible unless something prints it.
    // eslint-disable-next-line no-console -- a test reporting what it measured
    console.log(`  drift comparison: ${breakdown}`);
  });

  it("records a digest for every artifact, so no line escapes the comparison", () => {
    // A missing digest makes every rule above vacuous for that line, which is the easiest
    // way to switch this check off by accident.
    const missing = Object.entries(budget.artifacts)
      .filter(([, line]) => !line.measured_sha256 || !line.measured_sha256_normalised)
      .map(([key]) => key);
    expect(missing).toEqual([]);
  });

  it("gives every not-byte-reproducible exemption a reason", () => {
    // An exemption is the one place a recorded size is never checked exactly, so it is the
    // one place that needs an argument rather than an entry.
    for (const [key, reason] of Object.entries(budget.not_byte_reproducible)) {
      expect(
        reason.length,
        `${key} is exempt from the exact check with no reason given`,
      ).toBeGreaterThan(40);
      expect(budget.artifacts, `${key} is exempt but is not an artifact`).toHaveProperty(key);
    }
  });
});

describe("normalising generated engine hashes out of the page digest", () => {
  // WHY THIS EXISTS: the page's CSP names every engine by its content-hashed URL, so ANY
  // engine rebuild changes index.html. CI proved it — `qpdf.wasm` is built from source,
  // its hash differs there, and the check reported BOTH `qpdf.wasm` and `page` as changed.
  //
  // The alternative was exempting `page` from the exact check, which would have meant not
  // checking the line the design system lives on because of a cause belonging to a
  // different line. Normalising keeps the page exact for everything that is actually the
  // page's.
  //
  // It has to hide the right thing and only the right thing, so both directions are here.

  const asBuffer = (text: string) => Buffer.from(text, "utf8");
  const digest = (text: string, path = "index.html") =>
    normaliseEngineHashes(asBuffer(text), path).toString("utf8");

  it("hides a changed engine hash", () => {
    const before = digest("connect-src /engines/qpdf.37e20b4683cf834a.wasm");
    const after = digest("connect-src /engines/qpdf.0123456789abcdef.wasm");
    expect(before).toBe(after);
  });

  it("does NOT hide a change to the page's own content", () => {
    // The near-miss. A normalisation that swallowed real edits would make the exact check
    // vacuous for the one artifact it is being kept exact for.
    expect(digest("<h1>Your files stay here</h1>")).not.toBe(digest("<h1>Upload your files</h1>"));
  });

  it("hides a Vite asset hash quoted in MARKUP, where it is only a name", () => {
    // A script chunk's name is a hash of its CONTENT, and a bundled script is not
    // byte-reproducible across architectures — so without this the markup inherits the
    // JavaScript's unreproducibility through a filename. Measured three times in CI.
    //
    // It is safe because of the split in `budgetKey`: the scripts have their own budget
    // line and their own digest, so a real island change shows up there rather than
    // disappearing. What is given up is a pure rename with identical content.
    expect(digest('<link href="/_astro/index.-sYAk8S9.css">')).toBe(
      digest('<link href="/_astro/index.BkQBePuw.css">'),
    );
  });

  it("does NOT hide a Vite asset hash outside markup", () => {
    // The near-miss, and the boundary of the rule above. Inside a script a hashed name is
    // what the module actually imports; hiding it there would let an import be repointed
    // with the digest unchanged.
    expect(digest('import"./render.Dy18q9u-.js"', "_astro/island.js")).not.toBe(
      digest('import"./render.BkQBePuw.js"', "_astro/island.js"),
    );
  });

  it("still notices a markup change that is not a hash", () => {
    // And the complement: normalising names must not leave the markup itself unwatched.
    expect(digest('<link href="/_astro/index.-sYAk8S9.css"><h1>a</h1>')).not.toBe(
      digest('<link href="/_astro/index.-sYAk8S9.css"><h1>b</h1>'),
    );
  });

  it("leaves binaries alone", () => {
    // A .wasm could contain sixteen hex bytes by coincidence, and rewriting them would
    // corrupt the one digest that is supposed to be exact.
    const bytes = Buffer.from(".0123456789abcdef.", "utf8");
    expect(normaliseEngineHashes(bytes, "engines/qpdf.wasm")).toEqual(bytes);
  });
});

describe("the drift probe itself", () => {
  // Every rule, against planted input, on every run. Without this the block above is only
  // ever exercised against a recording that matches — which is to say, never exercised.

  const bytes = { raw: 100, brotli: 50, sha256: "aaaa", sha256Normalised: "nnnn" };
  const base = { notByteReproducible: {}, tolerance: 0.02 };

  it("passes a recording that matches the build", () => {
    expect(
      driftFindings({
        ...base,
        recorded: {
          a: {
            measured_raw: 100,
            measured_brotli: 50,
            measured_sha256: "aaaa",
            measured_sha256_normalised: "nnnn",
          },
        },
        live: { a: bytes },
      }),
    ).toEqual([]);
  });

  it("catches a recording whose size drifted while the bytes did not", () => {
    // THE EXACT FAILURE THIS EXISTS FOR: same bytes, a different recorded number. 236 of
    // them, last time, and every other test passed.
    const findings = driftFindings({
      ...base,
      recorded: {
        a: {
          measured_raw: 100,
          measured_brotli: 49,
          measured_sha256: "aaaa",
          measured_sha256_normalised: "nnnn",
        },
      },
      live: { a: bytes },
    });
    expect(findings).toHaveLength(1);
    expect(findings[0]).toMatchObject({ kind: "false-record", key: "a", field: "brotli" });
    expect(explain(findings[0])).toContain("IDENTICAL");
  });

  it("catches a one-byte drift, because there is no tolerance when nothing changed", () => {
    const findings = driftFindings({
      ...base,
      recorded: {
        a: {
          measured_raw: 99,
          measured_brotli: 50,
          measured_sha256: "aaaa",
          measured_sha256_normalised: "nnnn",
        },
      },
      live: { a: bytes },
    });
    expect(findings.map((f) => f.kind)).toEqual(["false-record"]);
  });

  it("catches a recording with no digest rather than passing it", () => {
    const findings = driftFindings({
      ...base,
      recorded: { a: { measured_raw: 100, measured_brotli: 50 } },
      live: { a: bytes },
    });
    expect(findings.map((f) => f.kind)).toEqual(["no-digest"]);
  });

  it("catches an artifact whose bytes changed without being declared unreproducible", () => {
    const findings = driftFindings({
      ...base,
      recorded: {
        a: {
          measured_raw: 100,
          measured_brotli: 50,
          measured_sha256: "bbbb",
          measured_sha256_normalised: "mmmm",
        },
      },
      live: { a: bytes },
    });
    expect(findings.map((f) => f.kind)).toEqual(["undeclared-rebuild"]);
  });

  it("tolerates a declared unreproducible artifact moving a little", () => {
    expect(
      driftFindings({
        recorded: {
          a: {
            measured_raw: 100,
            measured_brotli: 50,
            measured_sha256: "bbbb",
            measured_sha256_normalised: "mmmm",
          },
        },
        live: { a: { raw: 101, brotli: 51, sha256: "aaaa", sha256Normalised: "nnnn" } },
        notByteReproducible: { a: "because" },
        tolerance: 0.02,
      }),
    ).toEqual([]);
  });

  it("still catches a declared unreproducible artifact moving a lot", () => {
    // The bound on the one window this check cannot close exactly. Without it, "not byte
    // reproducible" would mean "not checked", which is an open door rather than a
    // declared width.
    const findings = driftFindings({
      recorded: {
        a: {
          measured_raw: 100,
          measured_brotli: 50,
          measured_sha256: "bbbb",
          measured_sha256_normalised: "mmmm",
        },
      },
      live: { a: { raw: 200, brotli: 100, sha256: "aaaa", sha256Normalised: "nnnn" } },
      notByteReproducible: { a: "because" },
      tolerance: 0.02,
    });
    expect(findings.map((f) => f.kind)).toEqual(["drift"]);
    expect(explain(findings[0])).toContain("tolerance");
  });

  it("tolerates a size wobble when only the engine hashes differ", () => {
    // THE CASE CI TAUGHT. A normalised match is NOT identical bytes: the page's CSP quotes
    // every engine's content hash, and two hashes do not compress identically. Asserting
    // exact equality here reported `the bytes are IDENTICAL ... 21652 vs 21650`, which was
    // false in its first four words.
    expect(
      driftFindings({
        ...base,
        recorded: {
          a: {
            measured_raw: 100,
            measured_brotli: 50,
            measured_sha256: "aaaa",
            measured_sha256_normalised: "nnnn",
          },
        },
        live: { a: { raw: 102, brotli: 52, sha256: "DIFFERENT", sha256Normalised: "nnnn" } },
      }),
    ).toEqual([]);
  });

  it("still catches a real change hiding behind an engine-hash difference", () => {
    // The near-miss for the case above. If the bound were a percentage, or absent, an edit
    // to the page could ride along with an engine rebuild unnoticed -- and an engine
    // rebuild happens on every CI run, so that would be a permanent blind spot.
    const findings = driftFindings({
      ...base,
      recorded: {
        a: {
          measured_raw: 100,
          measured_brotli: 50,
          measured_sha256: "aaaa",
          measured_sha256_normalised: "nnnn",
        },
      },
      live: {
        a: {
          raw: 900,
          brotli: 50 + HASH_COUPLING_BYTES + 1,
          sha256: "x",
          sha256Normalised: "nnnn",
        },
      },
    });
    expect(findings.map((f) => f.kind)).toEqual(["hash-coupled-drift"]);
    expect(explain(findings[0])).toContain("Something else changed too");
  });

  it("treats a changed normalised digest as a real change, not a hash wobble", () => {
    // An edit to the page's own markup moves the normalised digest, so it can never be
    // explained away as substitution noise however small it is.
    const findings = driftFindings({
      ...base,
      recorded: {
        a: {
          measured_raw: 100,
          measured_brotli: 50,
          measured_sha256: "aaaa",
          measured_sha256_normalised: "nnnn",
        },
      },
      live: { a: { raw: 100, brotli: 50, sha256: "x", sha256Normalised: "CHANGED" } },
    });
    expect(findings.map((f) => f.kind)).toEqual(["undeclared-rebuild"]);
  });

  it("catches a recording missing only the normalised digest", () => {
    const findings = driftFindings({
      ...base,
      recorded: { a: { measured_raw: 100, measured_brotli: 50, measured_sha256: "aaaa" } },
      live: { a: bytes },
    });
    expect(findings.map((f) => f.kind)).toEqual(["no-digest"]);
  });

  it("classifies each of the four outcomes it can report", () => {
    // The classifier is what makes the breakdown above trustworthy. If it reported "exact"
    // for everything, the line would read as a strong pass forever.
    const recorded = {
      same: {
        measured_raw: 1,
        measured_brotli: 1,
        measured_sha256: "a",
        measured_sha256_normalised: "n",
      },
      coupled: {
        measured_raw: 1,
        measured_brotli: 1,
        measured_sha256: "a",
        measured_sha256_normalised: "n",
      },
      real: {
        measured_raw: 1,
        measured_brotli: 1,
        measured_sha256: "a",
        measured_sha256_normalised: "n",
      },
      bare: { measured_raw: 1, measured_brotli: 1 },
      gone: {
        measured_raw: 1,
        measured_brotli: 1,
        measured_sha256: "a",
        measured_sha256_normalised: "n",
      },
    };
    const live = {
      same: { raw: 1, brotli: 1, sha256: "a", sha256Normalised: "n" },
      coupled: { raw: 1, brotli: 1, sha256: "DIFFERENT", sha256Normalised: "n" },
      real: { raw: 1, brotli: 1, sha256: "a", sha256Normalised: "DIFFERENT" },
      bare: { raw: 1, brotli: 1, sha256: "a", sha256Normalised: "n" },
    };
    expect(classify({ recorded, live })).toEqual({
      same: "exact",
      coupled: "hash-coupled",
      real: "changed",
      bare: "no-digest",
      gone: "absent",
    });
  });

  it("says nothing about an artifact the build no longer contains", () => {
    // That is a different test's job, in both directions, and duplicating it here would
    // mean two places to update when the payload changes.
    expect(
      driftFindings({
        ...base,
        recorded: {
          gone: {
            measured_raw: 1,
            measured_brotli: 1,
            measured_sha256: "aaaa",
            measured_sha256_normalised: "nnnn",
          },
        },
        live: {},
      }),
    ).toEqual([]);
  });
});
