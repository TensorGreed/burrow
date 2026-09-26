#!/usr/bin/env python3
"""Tie each gitignored build artifact to the tree it was built from, and refuse a stale one.

WHY THIS EXISTS (#149)

`engines/vendor/wasm/` and `bindings/burrow-wasm/pkg*/` are gitignored and rebuilt per
machine, and until this file nothing tied either one to the source it came from. An artifact
left behind by another branch was indistinguishable from a current one, so every measurement
taken against it measured the wrong tree. Five times, each costing a failed sweep:

    #128   pkg/ from another branch       the size budget reported the other branch's figure
    #130   the same, after a switch       noticed because the number was recognisable
    #145   vendor/wasm/lib/qpdf.wasm      two exports the branch did not declare
    #201   vendor/wasm/lib/qpdf.wasm      #200's export while the branch was based on main
    #201   apps/web/dist                  a harness build left by #200's e2e run

Between those branches the artifact was swapped by hand and checked by digest. That is the
discipline this replaces: **a habit that has failed five times is not a control.**

WHAT A STAMP IS

Each build writes `<output>/.build-stamp`: a record of every INPUT the build consumed -- a
sha256 per file, and the values that are not files (the pinned emsdk, the qpdf export list,
the rustc and wasm-pack versions, the exact build command). `check` recomputes the record from
the tree as it is now and refuses on any difference, naming the inputs that differ.

  * THE OUTPUT IS NOT HASHED. The question is whether the output is stale relative to its
    inputs; hashing the output says only that it is the output.
  * THE STAMP IS BESIDE THE ARTIFACT, NOT IN IT. `qpdf.wasm` is compared byte-for-byte
    against a digest in `apps/web/size-budget.json`, and a varying stamp inside it would break
    that.
  * IT REFUSES; IT DOES NOT WARN. All five incidents had a person reading output and deciding
    it looked fine.
  * THE RECORD IS TAKEN BEFORE THE BUILD AND COMPARED AFTER IT. A tree that moved while the
    build ran produced an artifact from neither state, so no stamp is written for it.
  * A FAILED BUILD LEAVES NO STAMP. The old one is removed before the build starts, so an
    absent stamp and a failed build read the same way: rebuild.

WHAT IS AN INPUT, AND WHERE THAT IS OVER-INCLUSIVE ON PURPOSE

  engines-wasm   `engines/{pins.toml,pins.sh,fetch.sh,build-wasm.sh}`, plus the pinned emsdk
                 version and the qpdf export list read out of them (named separately so a
                 refusal can say `+_qpdf_oh_set_array_item` rather than "build-wasm.sh
                 changed"). The emsdk value is the PIN, not `emcc --version`: build-wasm.sh
                 refuses to build on any other version, so the two cannot differ in an
                 artifact that exists.
  pkg,           every file under `bindings/burrow-wasm` and each workspace crate it depends on
  pkg-render     (build and normal dependencies, every target, every feature), the root
                 `Cargo.toml`, `Cargo.lock` and `rust-toolchain.toml`, and the `rustc` and
                 `wasm-pack` versions and the build command. A crate's `tests/` cannot change
                 its library and is included anyway; the whole `Cargo.lock` is included rather
                 than the entries that matter. Both cost a rebuild that was not strictly
                 needed, never a stale artifact passing -- the direction to be wrong in.

THE CACHE KEY MUST AGREE, AND THAT IS CHECKED ON EVERY RUN

CI restores `engines/vendor` from a cache keyed on a hash of named files, and a restored stamp
is only current if the key covers every file the stamp records. So this refuses if the
`wasm-engines-` key in `.github/actions/build-web-payload/action.yml` names anything other than
engines-wasm's input files plus this file. Were the key narrower, a warm cache would restore a
stamp for a tree that has since moved and every web job would refuse; were it wider, nothing
would be wrong but the rule would no longer be stated in one place. This file is in the key
because the input set is defined here: changing it must rebuild, or the cached stamp would lack
the new input and refuse.

USAGE

  tools/build-stamp.py check <artifact>... [--stamp PATH]   refuse unless each stamp is current
  tools/build-stamp.py wrap <artifact> [--stamp PATH] -- <build command...>
  tools/build-stamp.py begin <artifact>                      print the input record (JSON)
  tools/build-stamp.py commit <artifact> <record> [--stamp PATH]
  tools/build-stamp.py --probe                               the per-rule probes, and nothing else

`begin`/`commit` are `wrap` split in two, for `engines/build-wasm.sh`, which is itself the build
and cannot be wrapped from inside. `--stamp` reads or writes a stamp somewhere other than beside
the artifact; it exists so `tools/test-build-stamp.sh` can plant a stale stamp without touching
a real one. The inputs are always read from this repository.

A CONSUMER GUARDS ITSELF WITH THE LITERAL `build-stamp.py check <artifacts>`, and
`tools/ci-local.py` derives which jobs read which artifact by finding that literal in what
each job runs. Spelled any other way, the guard still works where it is, and the sweep's
preflight no longer sees it.
"""

from __future__ import annotations

import hashlib
import json
import re
import subprocess
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SELF = "tools/build-stamp.py"
STAMP_NAME = ".build-stamp"

# Bumped whenever what a stamp records changes shape. A stamp of another format is refused by
# name rather than diffed, because its keys would read as every input having changed.
FORMAT = 1

CACHE_KEY_FILE = ".github/actions/build-web-payload/action.yml"
CACHE_KEY_PREFIX = "wasm-engines-"

_WASM_PACK = ["wasm-pack", "build", "bindings/burrow-wasm", "--target", "no-modules"]

ARTIFACTS: dict[str, dict] = {
    "engines-wasm": {
        "output": "engines/vendor/wasm",
        "files": [
            "engines/pins.toml",
            "engines/pins.sh",
            "engines/fetch.sh",
            "engines/build-wasm.sh",
        ],
        "rebuild": "engines/fetch.sh && engines/build-wasm.sh",
    },
    "pkg": {
        "output": "bindings/burrow-wasm/pkg",
        "crate": "bindings/burrow-wasm",
        "files": ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"],
        "command": [*_WASM_PACK, "--out-dir", "pkg", "--release"],
        "rebuild": "tools/ci-local.py --only wasm-pack",
    },
    "pkg-render": {
        "output": "bindings/burrow-wasm/pkg-render",
        "crate": "bindings/burrow-wasm",
        "files": ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml"],
        "command": [
            *_WASM_PACK,
            "--out-dir",
            "pkg-render",
            "--release",
            "--",
            "--no-default-features",
            "--features",
            "render",
        ],
        "rebuild": "tools/ci-local.py --only wasm-pack",
    },
}


class Refusal(Exception):
    """A reason not to trust, or not to write, a stamp. The message is the whole report."""


# --- reading the inputs -------------------------------------------------------------------


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def qpdf_exports(build_script: str) -> list[str] | None:
    """The `_symbol`s in build-wasm.sh's `qpdf_exports='...'` block, or None if there is none.

    Only inside the block: build-wasm.sh quotes symbol names in its comments too.
    """
    block = re.search(r"^qpdf_exports='(.*?)'", build_script, re.DOTALL | re.MULTILINE)
    if block is None:
        return None
    return sorted(set(re.findall(r'"(_\w+)"', block.group(1))))


def pinned_emsdk(pins_text: str) -> str | None:
    try:
        version = tomllib.loads(pins_text).get("emsdk", {}).get("version")
    except tomllib.TOMLDecodeError:
        return None
    return version if isinstance(version, str) and version else None


def cache_key_files(action_text: str) -> list[str] | None:
    """The files hashed into the `wasm-engines-` cache key, or None if no such key is found.

    A commented-out key is not a key, and the native `engines-` key is a different cache.
    """
    for line in action_text.splitlines():
        if line.lstrip().startswith("#"):
            continue
        found = re.search(rf"\bkey:\s*{CACHE_KEY_PREFIX}.*?hashFiles\(([^)]*)\)", line)
        if found:
            return sorted(re.findall(r"'([^']+)'", found.group(1)))
    return None


def crate_closure(crate: str, repo: Path = REPO) -> list[str]:
    """`crate` and every workspace crate it depends on, as repository-relative directories.

    Normal and build dependencies, under every target and every feature: the union, because
    the two wasm builds enable different features and neither may miss a crate. Not
    dev-dependencies, which no library build compiles.
    """
    root = tomllib.loads((repo / "Cargo.toml").read_text())
    workspace_deps = root.get("workspace", {}).get("dependencies", {})

    def path_dependencies(directory: str) -> set[str]:
        manifest = tomllib.loads((repo / directory / "Cargo.toml").read_text())
        tables = [manifest.get("dependencies", {}), manifest.get("build-dependencies", {})]
        for target in manifest.get("target", {}).values():
            tables += [target.get("dependencies", {}), target.get("build-dependencies", {})]
        found: set[str] = set()
        for table in tables:
            for name, spec in table.items():
                if not isinstance(spec, dict):
                    continue
                if "path" in spec:
                    where = (repo / directory / spec["path"]).resolve()
                elif spec.get("workspace") is True:
                    inherited = workspace_deps.get(spec.get("package", name), {})
                    if not isinstance(inherited, dict) or "path" not in inherited:
                        continue
                    where = (repo / inherited["path"]).resolve()
                else:
                    continue
                found.add(where.relative_to(repo.resolve()).as_posix())
        return found

    seen: set[str] = set()
    stack = [crate]
    while stack:
        directory = stack.pop()
        if directory in seen:
            continue
        seen.add(directory)
        stack.extend(path_dependencies(directory) - seen)
    return sorted(seen)


def tree_files(directories: list[str], repo: Path = REPO) -> list[str]:
    """Every tracked or untracked-but-not-ignored file under `directories`.

    `--exclude-standard` is what keeps `pkg/` out of the digest of the crate that builds it. A
    tracked file deleted in the working tree is skipped: it is not an input any more.
    """
    try:
        listed = subprocess.run(
            ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *directories],
            cwd=repo,
            capture_output=True,
            check=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        raise Refusal(f"cannot list the source tree with git ({error}), so nothing can be compared")
    return sorted({p for p in listed.decode().split("\0") if p and (repo / p).is_file()})


def tool_version(argv: list[str]) -> str:
    try:
        done = subprocess.run(argv, cwd=REPO, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.SubprocessError) as error:
        return f"unavailable ({argv[0]}: {error.__class__.__name__})"
    first = (done.stdout or done.stderr).strip().splitlines()
    return first[0] if done.returncode == 0 and first else f"unavailable ({argv[0]} exit {done.returncode})"


def compute(artifact: str) -> dict[str, str]:
    """What `artifact`'s build consumes, read from the tree as it is now."""
    spec = ARTIFACTS[artifact]
    record: dict[str, str] = {}
    files = list(spec["files"])
    if "crate" in spec:
        files += tree_files(crate_closure(spec["crate"]))
    for name in files:
        path = REPO / name
        if not path.is_file():
            raise Refusal(f"{artifact}: its input {name} does not exist")
        record[f"file:{name}"] = sha256(path)

    if artifact == "engines-wasm":
        exports = qpdf_exports((REPO / "engines/build-wasm.sh").read_text())
        emsdk = pinned_emsdk((REPO / "engines/pins.toml").read_text())
        if exports is None or emsdk is None:
            raise Refusal(
                "engines-wasm: could not read "
                + ("the qpdf export list from engines/build-wasm.sh" if exports is None else "[emsdk] version from engines/pins.toml")
            )
        record["value:qpdf exports"] = " ".join(exports)
        record["value:emsdk (pinned)"] = emsdk
    else:
        record["value:rustc"] = tool_version(["rustc", "-V"])
        record["value:wasm-pack"] = tool_version(["wasm-pack", "-V"])
        record["value:command"] = " ".join(spec["command"])
    return record


# --- comparing ----------------------------------------------------------------------------


def differences(recorded: dict[str, str], current: dict[str, str]) -> list[str]:
    """Each input that differs between the build's record and the tree, named. Empty if none."""
    lines: list[str] = []

    def listing(label: str, names: list[str]) -> None:
        if not names:
            return
        shown = ", ".join(names[:8]) + (f" (+{len(names) - 8} more)" if len(names) > 8 else "")
        lines.append(f"{label} ({len(names)}): {shown}")

    def files(keys) -> list[str]:
        return sorted(k[len("file:"):] for k in keys if k.startswith("file:"))

    changed = [k for k in recorded.keys() & current.keys() if recorded[k] != current[k]]
    listing("changed since the build", files(changed))
    listing("new since the build", files(current.keys() - recorded.keys()))
    listing("gone since the build", files(recorded.keys() - current.keys()))

    for key in sorted(k for k in changed if k.startswith("value:")):
        name = key[len("value:"):]
        if name == "qpdf exports":
            was, now = set(recorded[key].split()), set(current[key].split())
            delta = [f"+{s}" for s in sorted(now - was)] + [f"-{s}" for s in sorted(was - now)]
            lines.append(f"{name} differ from the build's: {' '.join(delta)}")
        else:
            lines.append(f"{name}: built with {recorded[key]!r}, now {current[key]!r}")
    for key in sorted(k for k in current.keys() - recorded.keys() if k.startswith("value:")):
        lines.append(f"{key[len('value:'):]}: not recorded by the build (the stamp predates it)")
    for key in sorted(k for k in recorded.keys() - current.keys() if k.startswith("value:")):
        lines.append(f"{key[len('value:'):]}: recorded by the build, no longer an input")
    return lines


def stamp_path(artifact: str, override: str | None) -> Path:
    return Path(override) if override else REPO / ARTIFACTS[artifact]["output"] / STAMP_NAME


def check(artifact: str, override: str | None = None) -> tuple[bool, str]:
    """`(current, report)`. The report names what was examined, or what differs and the fix."""
    spec = ARTIFACTS[artifact]
    path = stamp_path(artifact, override)
    fix = f"rebuild it: {spec['rebuild']}"
    if not path.is_file():
        where = spec["output"]
        why = "there is no build" if not (REPO / where).exists() else "the build left no stamp (it failed, or predates #149)"
        return False, f"{artifact}: no stamp at {path.relative_to(REPO) if path.is_relative_to(REPO) else path} -- {why}; {fix}"
    try:
        stamp = json.loads(path.read_text())
        recorded = stamp["inputs"]
        fmt, named = stamp["format"], stamp["artifact"]
    except (OSError, ValueError, KeyError, TypeError) as error:
        return False, f"{artifact}: the stamp is unreadable ({error.__class__.__name__}); {fix}"
    if fmt != FORMAT:
        return False, f"{artifact}: the stamp is format {fmt!r} and this tool reads format {FORMAT}; {fix}"
    if named != artifact:
        return False, f"{artifact}: the stamp there is for {named!r}; {fix}"
    try:
        current = compute(artifact)
    except Refusal as refusal:
        return False, str(refusal)
    found = differences(recorded, current)
    if found:
        return False, "\n".join(
            [f"{artifact}: built from a different tree than this one --", *(f"    {l}" for l in found), f"    {fix}"]
        )
    n_files = sum(1 for k in current if k.startswith("file:"))
    return True, (
        f"{artifact}: current -- {len(current)} input(s) examined, {len(recorded)} recorded "
        f"({n_files} file(s), {len(current) - n_files} value(s))"
    )


def write(artifact: str, record: dict[str, str], override: str | None) -> Path:
    path = stamp_path(artifact, override)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"format": FORMAT, "artifact": artifact, "inputs": record}, indent=1, sort_keys=True) + "\n")
    return path


def settled(artifact: str, before: dict[str, str]) -> None:
    """Refuse if the tree moved while the build ran: that artifact is from neither state."""
    moved = differences(before, compute(artifact))
    if moved:
        raise Refusal(
            "\n".join(
                [f"{artifact}: the inputs changed while it was building, so no stamp was written --",
                 *(f"    {l}" for l in moved),
                 f"    rebuild it on a still tree: {ARTIFACTS[artifact]['rebuild']}"]
            )
        )


# --- the rules' own probes, every run -----------------------------------------------------


def probes() -> tuple[int, list[str]]:
    """`(how many ran, the ones that failed)`. A rule that matches nothing passes everything."""
    failed: list[str] = []
    ran = 0

    def probe(name: str, ok: bool) -> None:
        nonlocal ran
        ran += 1
        if not ok:
            failed.append(name)

    base = {"file:a": "1", "file:b": "2", "value:v": "x", "value:qpdf exports": "_a _b"}
    probe("an identical record differs in nothing", differences(base, dict(base)) == [])
    probe("a changed file is named", any("changed" in l and ": b" in l for l in differences(base, {**base, "file:b": "3"})))
    probe("a new file is named", any("new since" in l and ": c" in l for l in differences(base, {**base, "file:c": "4"})))
    probe(
        "a vanished file is named",
        any("gone since" in l and ": a" in l for l in differences(base, {k: v for k, v in base.items() if k != "file:a"})),
    )
    probe("a changed value is named", any(l.startswith("v: built with 'x'") for l in differences(base, {**base, "value:v": "y"})))
    probe(
        "an export added since the build is named by symbol",
        any("+_c" in l for l in differences(base, {**base, "value:qpdf exports": "_a _b _c"})),
    )

    script = "# \"_in_a_comment\"\nqpdf_exports='\"_malloc\",\n  \"_qpdf_init\"'\n\"_after_the_block\"\n"
    probe("the export list is read from its block", qpdf_exports(script) == ["_malloc", "_qpdf_init"])
    probe("no export block reads as none, not as empty", qpdf_exports("exports='\"_malloc\"'") is None)

    probe("the emsdk pin is read from [emsdk]", pinned_emsdk('[emsdk]\nversion = "6.0.9"\n') == "6.0.9")
    probe("a version under another table is not the emsdk pin", pinned_emsdk('[qpdf]\nversion = "12"\n') is None)

    action = (
        "      # key: wasm-engines-x-${{ hashFiles('commented.sh') }}\n"
        "        key: engines-x-${{ hashFiles('native.sh') }}\n"
        "        key: wasm-engines-${{ runner.os }}-${{ hashFiles('b.sh', 'a.toml') }}\n"
    )
    probe("the wasm cache key's files are read", cache_key_files(action) == ["a.toml", "b.sh"])
    probe("a commented or native key is not the wasm key", cache_key_files(action.rsplit("\n", 2)[0]) is None)

    closure = crate_closure("bindings/burrow-wasm")
    probe("the binding's closure reaches burrow-types", "core/burrow-types" in closure)
    probe("the binding's closure does not reach the uniffi binding", "bindings/burrow-ffi" not in closure)
    listed = tree_files(["core/burrow-types"])
    probe("a crate's tree lists its own manifest", "core/burrow-types/Cargo.toml" in listed)
    probe("a crate's tree lists nothing outside it", all(p.startswith("core/burrow-types/") for p in listed))
    return ran, failed


def cache_key_agrees() -> str | None:
    """None if the CI cache key covers exactly engines-wasm's input files plus this file."""
    text = (REPO / CACHE_KEY_FILE).read_text()
    keyed = cache_key_files(text)
    expected = sorted([*ARTIFACTS["engines-wasm"]["files"], SELF])
    if keyed is None:
        return f"no `key: {CACHE_KEY_PREFIX}...hashFiles(...)` line in {CACHE_KEY_FILE}"
    if keyed != expected:
        missing = sorted(set(expected) - set(keyed))
        extra = sorted(set(keyed) - set(expected))
        return (
            f"the {CACHE_KEY_PREFIX} cache key in {CACHE_KEY_FILE} does not hash exactly what the "
            f"stamp records: missing {missing or 'nothing'}, extra {extra or 'nothing'}. A warm cache "
            "would restore a stamp for a tree that has since moved."
        )
    return None


# --- the command line ---------------------------------------------------------------------


def main(argv: list[str]) -> int:
    ran, failed = probes()
    if failed:
        print(f"build-stamp: REFUSED -- {len(failed)} of {ran} rule probe(s) failed:", file=sys.stderr)
        for name in failed:
            print(f"  - {name}", file=sys.stderr)
        return 2
    disagreement = cache_key_agrees()
    if disagreement:
        print(f"build-stamp: REFUSED -- {disagreement}", file=sys.stderr)
        return 2
    if argv == ["--probe"]:
        print(f"build-stamp: {ran} rule probe(s) verified, and the cache key agrees")
        return 0

    command: list[str] = []
    if "--" in argv:
        split = argv.index("--")
        argv, command = argv[:split], argv[split + 1 :]
    override = None
    if "--stamp" in argv:
        index = argv.index("--stamp")
        if index + 1 >= len(argv):
            print("build-stamp: --stamp needs a path", file=sys.stderr)
            return 2
        override = argv[index + 1]
        argv = argv[:index] + argv[index + 2 :]
    if not argv:
        print(__doc__.split("USAGE", 1)[1].split("`begin`", 1)[0], file=sys.stderr)
        return 2
    verb, names = argv[0], argv[1:]
    artifacts = names[:1] if verb == "commit" else names
    if not artifacts or any(n not in ARTIFACTS for n in artifacts):
        print(f"build-stamp: {verb} needs artifact(s) from {', '.join(ARTIFACTS)}; got {names}", file=sys.stderr)
        return 2
    if override and len(names) > 1 and verb == "check":
        print("build-stamp: --stamp names one stamp, so check one artifact with it", file=sys.stderr)
        return 2

    try:
        if verb == "check":
            stale = 0
            for name in names:
                ok, report = check(name, override)
                print(f"build-stamp: {report}", file=sys.stdout if ok else sys.stderr)
                stale += not ok
            if stale:
                print(
                    f"build-stamp: REFUSED -- {stale} of {len(names)} artifact(s) do not match this tree. "
                    "Anything measured against them measures another tree.",
                    file=sys.stderr,
                )
                return 1
            return 0

        artifact = names[0]
        if verb == "begin":
            print(json.dumps(compute(artifact), sort_keys=True))
            return 0

        if verb == "commit":
            if len(names) != 2:
                print("build-stamp: commit <artifact> <record>", file=sys.stderr)
                return 2
            before = json.loads(Path(names[1]).read_text())
            settled(artifact, before)
            print(f"build-stamp: wrote {write(artifact, before, override)} ({len(before)} inputs)")
            return 0

        if verb == "wrap":
            expected = ARTIFACTS[artifact].get("command")
            if expected is None or command != expected:
                raise Refusal(
                    f"{artifact}: wrap runs exactly `{' '.join(expected) if expected else '(no wrapped build)'}`, "
                    f"and was given `{' '.join(command) or '(nothing)'}` -- a stamp must describe the build that ran"
                )
            stamp_path(artifact, override).unlink(missing_ok=True)
            before = compute(artifact)
            status = subprocess.run(command, cwd=REPO).returncode
            if status != 0:
                print(f"build-stamp: {artifact}: the build failed (exit {status}); no stamp written", file=sys.stderr)
                return status
            settled(artifact, before)
            print(f"build-stamp: wrote {write(artifact, before, override)} ({len(before)} inputs)")
            return 0
    except Refusal as refusal:
        print(f"build-stamp: REFUSED -- {refusal}", file=sys.stderr)
        return 1

    print(f"build-stamp: unknown verb {verb!r}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
