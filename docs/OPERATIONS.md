# Operations

Runbooks for the things only a person does. For the architecture of the deploy, see
[ADR 0024](adr/0024-how-burrow-is-deployed.md) and its 2026-10-05 deploy-on-tag amendment.

## Cutting a release (deploying the site)

Since deploy-on-tag, a merge to `main` **builds and verifies** the production payload and uploads
it as an artifact — it does **not** deploy. A push of an annotated **`v*`** tag is what deploys,
by reusing that verified artifact. So a release is one deliberate step, taken when you decide to
cut one.

> **Release history, and two tags that stand as records of fixed defects.**
> - **`v0.1.0`** (on `0aa1bd2`) **was never deployed.** Its `publish` refused on a stamp-check
>   defect — the precondition checked `apps/web/dist` for a `build-stamp` nothing wrote there, so
>   it refused every tag. Fixed by the literal SHA stamp (`release-sha.txt`) in PR #265.
> - **`v0.1.1`** (on `f834607`, deploy run `37495869245`) **deployed but was never signed.** The
>   site went live and verified (live `release-sha.txt` == the tag), but the `sign` job failed:
>   cosign v3's `sign-blob` had been called with flags cosign removed. Fixed by pinning cosign and
>   the `--bundle` invocation in PR #266, rehearsed on every main push so it cannot recur.
> - **`v0.1.2`** (on `f497910`, deploy run `37515670056`) is the **first complete release** —
>   deployed *and* signed. Verified both halves: live `release-sha.txt` == the tag and all live
>   files byte-identical to the build; and the GitHub Release carries the tarball, its `.bundle`
>   and the SBOM, with `cosign verify-blob --bundle` (identity `…deploy.yml@refs/tags/v0.1.2`,
>   issuer `token.actions.githubusercontent.com`) passing against the downloaded assets.
>
> **cosign version history (the "did it drift?" question).** It did not drift — there was nothing
> to drift from. **No release before `v0.1.2` was ever signed:** `v0.1.0` never deployed, and
> `v0.1.1`'s `sign` job failed before producing anything. `v0.1.1` ran the cosign-installer's
> **unpinned default, cosign v3.0.6**, whose `sign-blob` had dropped the v2 flags the invocation
> still used — the invocation was simply never valid for the version that ran, not a drift between
> versions. Since PR #266 cosign is **pinned to v3.0.6 by sha256** (`COSIGN_VERSION` /
> `COSIGN_SHA256_AMD64` in `deploy.yml`), so `v0.1.2` — the first signature that exists — was made
> by, and verifies against, that exact pinned version.

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
#    build stamps the payload with the commit it was built from (apps/web/dist/release-sha.txt),
#    served as text/plain, so the live origin serves /release-sha.txt and it must equal the SHA.
#    This reads the BODY, not the status code: a static host can answer a missing path 200+HTML.
tools/check-live-release-sha.sh https://notonlypdf.com "$SHA"

# And the whole live build must match the verified artifact (not just the stamp). Download the
# artifact publish deployed (by the run's id, never "latest") and compare the live origin to it.
BUILD_RUN=$(gh run list --commit "$SHA" --workflow deploy.yml --branch main \
              --json databaseId -q '.[0].databaseId')
rm -rf /tmp/burrow-deployed && gh run download "$BUILD_RUN" -n production-dist -D /tmp/burrow-deployed
tools/check-live-routes.py https://notonlypdf.com /tmp/burrow-deployed   # live == these bytes (incl. /release-sha.txt)

# 4. The signed release exists and verifies. The `sign` job publishes a GitHub Release with the
#    tarball, its single Sigstore bundle, and the SBOM. Confirm all three are attached and that the
#    signature verifies against the DOWNLOADED assets with the pinned cosign.
rm -rf /tmp/burrow-rel && gh release download "$VERSION" -D /tmp/burrow-rel
ls /tmp/burrow-rel   # expect burrow-web-$VERSION.tar.gz, .tar.gz.bundle, .tar.gz.sha256, burrow.cdx.json
( cd /tmp/burrow-rel && sha256sum -c "burrow-web-$VERSION.tar.gz.sha256" )
cosign verify-blob "/tmp/burrow-rel/burrow-web-$VERSION.tar.gz" \
  --bundle "/tmp/burrow-rel/burrow-web-$VERSION.tar.gz.bundle" \
  --certificate-identity "https://github.com/TensorGreed/burrow/.github/workflows/deploy.yml@refs/tags/$VERSION" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com

echo "deployed and signed: https://notonlypdf.com is serving $VERSION ($SHA), release assets verify"
```

Do not say "deployed" until `check-live-release-sha.sh` and `check-live-routes.py` pass; do not say
"released" until step 4 also passes. The first two say the live origin is serving this exact tag's
whole verified build; step 4 says the signed tarball, its bundle and the SBOM are published and the
signature verifies against the downloaded assets. Any of them failing means the live site or the
release is **not** this tag, whatever the run's exit code was.

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
