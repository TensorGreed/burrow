# Operations

Runbooks for the things only a person does. For the architecture of the deploy, see
[ADR 0024](adr/0024-how-burrow-is-deployed.md) and its 2026-10-05 deploy-on-tag amendment.

## Cutting a release (deploying the site)

Since deploy-on-tag, a merge to `main` **builds and verifies** the production payload and uploads
it as an artifact — it does **not** deploy. A push of an annotated **`v*`** tag is what deploys,
by reusing that verified artifact. So a release is one deliberate step, taken when you decide to
cut one.

### Before you tag

- **The commit must be on `main` and green.** The tag's commit has to be an ancestor of
  `origin/main` (merged), and both the **`ci`** run and the **`deploy` build** run for that
  commit's *main push* must have concluded success. `publish` re-checks all of this and refuses
  otherwise (`tools/check-release-preconditions.py`), but checking first saves a refused tag.
- **The commit must be recent enough.** `publish` deploys the artifact the main-push build
  uploaded, and that artifact is kept for **30 days** (`retention-days` on the `production-dist`
  upload in `.github/workflows/deploy.yml`). **Tagging a commit whose main-push build is older
  than 30 days refuses** — the artifact has expired and there is no rebuild fallback. The fix is
  not to loosen the check: it is to land a new commit on `main` (even an empty one is enough to
  produce a fresh build) and tag that.
- **The tag must be annotated.** A lightweight tag is a bare pointer and is refused; `git tag -a`
  (or `-s`) makes an annotated tag object.

### The command

Replace `vX.Y.Z` with the version and `<sha>` with the merged commit on `main` you are releasing
(default: the current `origin/main` tip).

```bash
set -euo pipefail
VERSION=vX.Y.Z
SHA=$(git rev-parse origin/main)     # or the specific merged commit

# 1. Annotated tag on the merged commit, and push it. Pushing the tag is what triggers publish.
git tag -a "$VERSION" "$SHA" -m "release $VERSION"
git push origin "$VERSION"

# 2. Watch the publish run for THIS tag to its conclusion. One run id, read the conclusion back
#    (an exit code is not an outcome — see CLAUDE.md).
RUN=$(gh run list --workflow deploy.yml --event push --branch "$VERSION" \
        --json databaseId -q '.[0].databaseId')
gh run watch "$RUN" --exit-status
gh run view "$RUN" --json conclusion,jobs -q '.conclusion'   # must print: success

# 3. Read the deployed bytes back from the LIVE site, and only then call it deployed.
#    "deployed" means the live origin serves the verified artifact for this exact tag — not that
#    a step exited 0. Download the artifact publish deployed (by the run's id, never "latest"),
#    then compare the live origin against it.
BUILD_RUN=$(gh run list --commit "$SHA" --workflow deploy.yml --branch main \
              --json databaseId -q '.[0].databaseId')
rm -rf /tmp/burrow-deployed && gh run download "$BUILD_RUN" -n production-dist -D /tmp/burrow-deployed

# The stamp must be current against the tagged commit's tree (the bytes ARE this commit's), and
# the live origin must serve exactly those bytes.
git -C . stash --include-untracked >/dev/null 2>&1 || true   # if your tree is dirty
git checkout "$VERSION" --quiet
python3 tools/build-stamp.py check /tmp/burrow-deployed       # stamp == the tagged tree
tools/check-live-routes.py https://notonlypdf.com /tmp/burrow-deployed   # live == these bytes
git checkout - --quiet

echo "deployed: https://notonlypdf.com is serving $VERSION ($SHA)"
```

Do not say "deployed" until both the stamp check and `check-live-routes.py` pass: the first says
the artifact is the tagged commit's bytes, the second says the live origin is serving them. Either
failing means the live site is **not** this tag, whatever the run's exit code was.

### If publish refuses

`publish` names the gate it refused on. Common ones:

- *not merged / not an ancestor of `origin/main`* — the tag is on a commit that is not on `main`.
  Tag a merged commit.
- *`ci` / `deploy build` not green on this commit* — the main push for this commit did not pass.
  Fix `main` and tag the new commit.
- *lightweight tag* — you used `git tag` without `-a`. Delete it (`git push origin :vX.Y.Z` and
  `git tag -d vX.Y.Z`) and re-tag with `-a`.
- *no artifact for the build run* — the build is older than the 30-day retention. Land a new
  commit on `main` and tag that.

## The `refs/tags/v*` ruleset

The deploy credential is reachable only through a `v*` tag, so who may create one is a security
control, not a convenience. The ruleset settings are recorded with this PR and applied in the
GitHub UI by the owner; see the PR description for the exact settings and the API read-back.
