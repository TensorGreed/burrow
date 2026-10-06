#!/usr/bin/env bash
# The ONE definition of how burrow signs and verifies a release blob with cosign.
#
#     tools/cosign-blob.sh sign   <artifact> <bundle-out> [extra cosign args...]
#     tools/cosign-blob.sh verify <artifact> <bundle>     [extra cosign args...]
#
# WHY A SHARED SCRIPT. The real release signs keyless (OIDC) on a `v*` tag; the sign-rehearsal
# signs with a throwaway key on every main push. They differ in HOW they authenticate, but they
# must agree on the OUTPUT SHAPE -- a single Sigstore `--bundle` -- and that shape is defined here,
# once. If it is wrong, the rehearsal goes red on the next main push, not first on a release.
#
# This exists because v0.1.1 shipped with the shape wrong and nothing caught it: `sign-blob` was
# called with cosign v2's `--output-signature`/`--output-certificate`, which cosign v3 removed, so
# it tried to write a bundle to an empty path and failed -- on the first tag that ever ran it, with
# no earlier release to compare against. The flags live in one place now, and the rehearsal runs
# them every push.
#
# The auth/tlog flags are passed by the caller as trailing args and are NOT shared:
#   keyless (real):    tools/cosign-blob.sh sign   a.tgz a.tgz.bundle
#                      tools/cosign-blob.sh verify a.tgz a.tgz.bundle \
#                        --certificate-identity <id> --certificate-oidc-issuer <issuer>
#   keyed (rehearsal): tools/cosign-blob.sh sign   dummy dummy.bundle --key cosign.key --tlog-upload=false
#                      tools/cosign-blob.sh verify dummy dummy.bundle --key cosign.pub --insecure-ignore-tlog=true

set -euo pipefail

usage() {
  echo "usage: tools/cosign-blob.sh sign   <artifact> <bundle-out> [extra cosign args...]" >&2
  echo "       tools/cosign-blob.sh verify <artifact> <bundle>     [extra cosign args...]" >&2
  exit 2
}

cmd="${1:-}"
[ -n "$cmd" ] || usage
shift

case "$cmd" in
  sign)
    artifact="${1:-}"; bundle="${2:-}"
    { [ -n "$artifact" ] && [ -n "$bundle" ]; } || usage
    shift 2
    # THE SHARED OUTPUT SHAPE. One Sigstore bundle carrying signature + certificate. No
    # `--output-signature`/`--output-certificate` (removed in cosign v3; that was the v0.1.1 bug).
    exec cosign sign-blob --yes --bundle "$bundle" "$@" "$artifact"
    ;;
  verify)
    artifact="${1:-}"; bundle="${2:-}"
    { [ -n "$artifact" ] && [ -n "$bundle" ]; } || usage
    shift 2
    exec cosign verify-blob --bundle "$bundle" "$@" "$artifact"
    ;;
  *)
    usage
    ;;
esac
