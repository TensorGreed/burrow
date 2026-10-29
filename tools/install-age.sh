#!/usr/bin/env bash
# Install the pinned `age` release into a directory, verified by sha256 before it is unpacked.
#
#   tools/install-age.sh <dir>        download, verify, unpack; prints the path of `age`
#   tools/install-age.sh --version    print the pinned version (tools/fuzz-seal-finding.sh reads it)
#
# `age` (https://github.com/FiloSottile/age, BSD-3-Clause) is a CI TOOL, not a dependency of any
# shipped artefact: nothing of it is linked into burrow, bundled or distributed. It encrypts fuzz
# findings so they can be kept without being published (owner decision (62)(1)). Pinned the way
# cosign is pinned in deploy.yml: by version AND sha256 of the release asset, so a re-tagged
# release or a swapped download is refused rather than run.
#
# The digests below were taken 2026-10-26 from the downloaded assets and cross-checked against the
# `digest` field GitHub's releases API reports for each asset. v1.3.2 was published 2026-08-29.
#
# AGE_RELEASE_BASE overrides the download base URL for tools/test-fuzz-seal-finding.sh, which
# points it at a tampered tarball (file://) to prove a mismatch is refused.
set -euo pipefail

AGE_VERSION="v1.3.2"
AGE_SHA256_LINUX_AMD64="cbe24006683f8eb669266162894b9a522a1af52f2665fbc63a4bb032ed26ac10"
AGE_SHA256_LINUX_ARM64="6b8dc4333c53a5a57c9e5834e3a48f92605d7154014cd07269ff3327db5d37f4"

if [ "${1:-}" = "--version" ]; then
  echo "$AGE_VERSION"
  exit 0
fi

dest="${1:?usage: install-age.sh <dir> | --version}"
case "$(uname -m)" in
  x86_64) arch="amd64"; want="$AGE_SHA256_LINUX_AMD64" ;;
  aarch64 | arm64) arch="arm64"; want="$AGE_SHA256_LINUX_ARM64" ;;
  *) echo "install-age: no pinned digest for $(uname -m)" >&2; exit 1 ;;
esac
base="${AGE_RELEASE_BASE:-https://github.com/FiloSottile/age/releases/download/$AGE_VERSION}"
asset="age-$AGE_VERSION-linux-$arch.tar.gz"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl --fail --silent --show-error --location --max-time 120 -o "$work/$asset" "$base/$asset"
got="$(sha256sum "$work/$asset" | cut -d' ' -f1)"
if [ "$got" != "$want" ]; then
  echo "install-age: $asset sha256 $got != pinned $want; refusing to unpack it" >&2
  exit 1
fi
mkdir -p "$dest"
tar -xzf "$work/$asset" -C "$dest" --strip-components=1 age/age age/age-keygen
reported="$("$dest/age" --version)"
if [ "$reported" != "$AGE_VERSION" ]; then
  echo "install-age: unpacked age reports $reported, not $AGE_VERSION" >&2
  exit 1
fi
echo "install-age: age $reported, release sha256 $got -- pinned" >&2
echo "$dest/age"
