# Operations

Runbooks for the things only a person does. For the architecture of the deploy, see
[ADR 0024](adr/0024-how-burrow-is-deployed.md) and its 2026-10-05 deploy-on-tag amendment.

## Cutting a release (deploying the site)

Since deploy-on-tag, a merge to `main` **builds and verifies** the production payload and uploads
it as an artifact — it does **not** deploy. A push of an annotated **`v*`** tag is what deploys,
by reusing that verified artifact. So a release is one deliberate step, taken when you decide to
cut one.

> **`v0.1.0` exists on `0aa1bd2` and was never deployed.** Its `publish` run refused on a
> stamp-check defect: the precondition checked `apps/web/dist` for a `build-stamp` nothing ever
> wrote there, so it refused every tag. Fixed by the literal SHA stamp (`.release-sha`) in
> PR #265. The tag is left in place as the record; the first real release is **`v0.1.1`**.

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

# 3. Read the deployed SHA back from the LIVE site, and only then call it deployed.
#    "deployed" means the live origin serves THIS tag's build — not that a step exited 0. The
#    build stamps the payload with the commit it was built from (apps/web/dist/.release-sha), so
#    the live origin serves /.release-sha, and it must equal the tagged SHA.
LIVE_SHA=$(curl -fsS https://notonlypdf.com/.release-sha)
[ "$LIVE_SHA" = "$SHA" ] || { echo "NOT deployed: live /.release-sha is '$LIVE_SHA', not $SHA"; exit 1; }

# And the whole live build must match the verified artifact (not just the stamp). Download the
# artifact publish deployed (by the run's id, never "latest") and compare the live origin to it.
BUILD_RUN=$(gh run list --commit "$SHA" --workflow deploy.yml --branch main \
              --json databaseId -q '.[0].databaseId')
rm -rf /tmp/burrow-deployed && gh run download "$BUILD_RUN" -n production-dist -D /tmp/burrow-deployed
tools/check-live-routes.py https://notonlypdf.com /tmp/burrow-deployed   # live == these bytes (incl. /.release-sha)

echo "deployed: https://notonlypdf.com is serving $VERSION ($SHA)"
```

Do not say "deployed" until both the `/.release-sha` match and `check-live-routes.py` pass: the
first says the live origin is serving this exact tag's build, the second says it is serving the
whole verified artifact. Either failing means the live site is **not** this tag, whatever the run's
exit code was.

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

## Repo settings the deploy boundary depends on (not visible in the tree)

The workflow enforces what it can, but three settings live in GitHub, not in this repo, and the
credential boundary rests on them. Confirm each when setting this up, and after any settings change:

1. **The `production` environment's deployment-branch policy must allow `v*` tags.** Before
   deploy-on-tag it was restricted to `main`; if it still is, every tag deploy is blocked by the
   environment (which is itself a correct fail-closed, but not the intended behaviour). Set it to
   allow the tag pattern `v*` (or the refs it needs), and keep it as the real backstop on which
   refs may use the environment.
2. **`CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` must be `production` *environment* secrets,
   not repository secrets.** As environment secrets they are reachable only by a job that names
   `environment: production` (the `publish` job), and the environment's branch/tag policy then
   governs when that is. As repo secrets they would be reachable from any job.
3. **The branch ruleset on `main` must require `ci` to pass.** The preconditions lean on "a green
   `ci` push-run on `main` means the commit passed CI"; that meaning comes from the branch
   ruleset, not from the deploy workflow.

## The `refs/tags/v*` ruleset

The deploy credential is reachable only through a `v*` tag, so who may create one is a security
control, not a convenience. The ruleset settings are recorded with this PR and applied in the
GitHub UI by the owner; see the PR description for the exact settings and the API read-back.
