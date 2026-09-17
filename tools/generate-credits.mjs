#!/usr/bin/env node
// Generate the website credits page's data from engines/licenses.toml.
//
// WHY THIS IS GENERATED, AND WHY IT IS A LICENCE OBLIGATION RATHER THAN A NICETY
//
// ADR 0008 and ADR 0010 admit three LICENCES that impose AFFIRMATIVE notice obligations
// binding executable-only distribution -- which is exactly what a wasm bundle is. They are
// carried by FOUR manifest entries, because libjpeg-turbo appears twice at different
// versions (PDFium's bundled copy, and the one we vendor):
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
// INPUTS ARE COMMITTED FILES ONLY, and that is ENFORCED rather than assumed -- see
// `resolveLicenceText`. Every `license_text` path must resolve inside engines/licences/ or
// docs/adr/licences/, never into engines/vendor/, which is gitignored. That is what lets
// `pnpm build` produce a complete credits page from a clean checkout, and what lets CI's web
// job (which stages only the wasm vendor prefix) build it at all.

import { readFile, mkdir, writeFile } from "node:fs/promises";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parseToml } from "./toml.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..");
const generatedDir = join(repo, "apps", "web", "src", "generated");

// The only two directories a `license_text` may name. Every committed licence text is in
// one of them (engines/licences/README.md says why the other exists).
//
// THIS IS A CONTAINMENT CHECK, NOT TIDINESS. `license_text` is a free-form string from
// engines/licenses.toml, and `path.join` collapses `..` -- so without this, one line of a
// file reviewers skim as a manifest rather than as code could make this script read any
// path on the machine and embed its contents verbatim into a PUBLISHED page. `prebuild`
// runs this on every build, and the output is 170 KB nobody reads line by line.
const LICENCE_ROOTS = [join(repo, "engines", "licences"), join(repo, "docs", "adr", "licences")];

/** Resolve a manifest `license_text` path, refusing anything outside {@link LICENCE_ROOTS}. */
function resolveLicenceText(name, relative) {
  const path = resolve(repo, relative);
  if (!LICENCE_ROOTS.some((root) => path === root || path.startsWith(root + sep))) {
    throw new Error(
      `${name}: license_text ${relative} resolves outside the committed licence ` +
        `directories. Licence text must live in engines/licences/ or docs/adr/licences/.`,
    );
  }
  return path;
}

async function main() {
  const manifestPath = join(repo, "engines", "licenses.toml");
  const manifest = parseToml(await readFile(manifestPath, "utf8"), "engines/licenses.toml");

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
      text = await readFile(resolveLicenceText(name, comp.license_text), "utf8");
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
    // Four entries carry an obligation today (three distinct licences -- libjpeg-turbo
    // appears twice). Zero means the manifest lost its `notice_required` fields, which
    // would silently produce a page that discharges nothing.
    throw new Error("engines/licenses.toml declares no notice_required obligations");
  }

  // --- SURFACES: each one credits ITS OWN artifacts -------------------------------------
  //
  // ADR 0008 names four surfaces, and they do not distribute the same things. Until spike
  // 0004 that distinction cost nothing, because the web shipped PDFium too. It does not now,
  // and the page went on crediting FreeType, HarfBuzz, ICU, lcms, OpenJPEG, Abseil and eight
  // more to a reader whose download contains none of them.
  //
  // Over-declaring breaches nothing, and that is exactly why it survived a security review as
  // a note rather than a finding. What it costs is the page's own claim -- that it "cannot
  // fall behind what we actually ship" -- which was true in one direction and loose in the
  // other. A notice page that credits things you did not receive is a page you cannot check.
  //
  // THE FULL SET STAYS IN THE PAYLOAD. Scoping happens at the render, so the Android (M3) and
  // iOS (M4) screens can scope to their own artifacts from the same data. FreeType's FTL §2
  // and HarfBuzz's MIT-Modern-Variant are still owed -- by the artifacts that carry PDFium --
  // and dropping them from the DATA rather than from one view is how an obligation gets lost.
  //
  // `artifacts`, NOT `linked_in`, AND THE DIFFERENCE IS A LICENCE BREACH.
  //
  // `artifacts` is "distributed with"; `linked_in` is "code was found in". A notice obligation
  // attaches to distribution, so `artifacts` is the field that answers it. And `linked_in` is
  // measurably the wrong answer here: scoping the web by it drops `zlib` and `libjpeg-turbo`,
  // both of which `tools/detect-engine-components.py` finds INSIDE the shipped `qpdf.wasm`
  // (libjpeg 3 hits, zlib 2 hits) -- and libjpeg-turbo carries the IJG affirmative notice. The
  // manifest's own header says why: it "does not populate or verify `linked_in`", and the
  // field "has never been re-derived per artifact".
  //
  // BOTH FIELDS ARE HAND-MAINTAINED, and this page's correctness now rests on `artifacts`
  // being right -- the neighbouring column of the same hand-maintained table we just found
  // stale. The detector already knows the truth by symbol inspection; issue #81 is to make it
  // fail when a component is found in a shipped artifact and not credited for it.
  // TWO ARTIFACTS SINCE ADR 0026, and the second one is CONDITIONAL. `qpdf.wasm` is in every
  // tool page's payload; `pdfium.wasm` is in the render bundle, which a page fetches only when
  // it needs a picture of a page. So a reader who merged two files has downloaded the first and
  // not the second.
  //
  // THE PAGE CREDITS BOTH, AND SAYS WHICH IS WHICH. Over-declaring breaches nothing -- an
  // obligation is discharged by the notice being reachable, not by it being minimal -- and the
  // alternative is worse in the direction that matters: a reader who DID render would be owed
  // FreeType's FTL §2 and HarfBuzz's MIT-Modern-Variant and would not have been given them,
  // because a page cannot know at build time what a visitor will do.
  //
  // What the page must not do is claim the reader downloaded all of it, which is the claim
  // spike 0004 left loose in the other direction. `conditional` below is what lets it name the
  // difference in its own words rather than leaving a reader to work it out.
  const WEB_ARTIFACTS = ["qpdf-wasm", "pdfium-wasm"];
  const WEB_CONDITIONAL_ARTIFACTS = ["pdfium-wasm"];
  const scopedTo = (ids) =>
    entries.filter((entry) => entry.artifacts.some((id) => ids.includes(id))).map((e) => e.name);
  const web = scopedTo(WEB_ARTIFACTS);
  // Components that arrive ONLY with the render bundle: in PDFium's set and not in the base
  // one. Derived rather than listed, so a component that starts shipping in both stops being
  // described as conditional without anyone having to notice.
  const always = scopedTo(WEB_ARTIFACTS.filter((id) => !WEB_CONDITIONAL_ARTIFACTS.includes(id)));
  const conditional = web.filter((name) => !always.includes(name));
  if (web.length === 0) {
    // A scope matching nothing would render an empty credits page, which discharges no
    // obligation and looks like a page with nothing to declare.
    throw new Error(
      `no component declares any of ${WEB_ARTIFACTS.join(", ")} in its artifacts, so the web ` +
        `credits page would be empty. Check the artifact ids in engines/licenses.toml`,
    );
  }

  const payload = {
    generatedBy: "tools/generate-credits.mjs from engines/licenses.toml",
    policyAdr: manifest.meta?.policy_adr ?? "docs/adr/0008-widened-licence-allowlist.md",
    lastAudited: manifest.meta?.last_audited ?? null,
    components: entries,
    surfaces: {
      web: {
        artifacts: WEB_ARTIFACTS,
        components: web,
        conditional: {
          artifacts: WEB_CONDITIONAL_ARTIFACTS,
          components: conditional,
        },
        why:
          "What a browser CAN download: qpdf.wasm on every tool page, and pdfium.wasm with " +
          "the render bundle a page fetches when it needs a picture of a page (ADR 0026). " +
          "`burrow_wasm_bg.wasm` is the Rust binding, whose dependencies are covered by " +
          "THIRD_PARTY_NOTICES.md rather than by this manifest. PDFium left the payload " +
          "entirely in spike 0004 and came back conditionally; `conditional` names the " +
          "components that arrive with it, because a page cannot know at build time whether " +
          "a given visitor will render anything.",
      },
    },
  };

  const out = `// GENERATED by tools/generate-credits.mjs. Do not edit.
//
// Derived from engines/licenses.toml, the same manifest tools/check-engine-licences.py
// gates in CI, so the credits page cannot drift from the licence check. See ADR 0008.
export const CREDITS = ${JSON.stringify(payload, null, 2)};
`;

  await mkdir(generatedDir, { recursive: true });
  await writeFile(join(generatedDir, "credits.js"), out);

  const webNotices = entries.filter(
    (e) => e.noticeRequired !== null && web.includes(e.name),
  ).length;
  console.log(
    `credits: ${entries.length} components (${entries.filter((e) => e.linked).length} linked), ` +
      `${notices.length} notice obligations, ` +
      `${entries.filter((e) => e.licenseText).length} licence texts; ` +
      `web surface: ${web.length} component(s), ${webNotices} notice(s)`,
  );
}

await main();
