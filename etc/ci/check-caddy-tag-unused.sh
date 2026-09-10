#!/usr/bin/env bash
# Check that an image tag is not already published in the registry.
#
# The tag-moved check in .github/workflows/caddy-image.yml compares TAG against
# the base branch. That sees a tag which moved, but not a tag which another
# build already claimed. Two pull requests bumping the same serial both move the
# tag and both pass that comparison; the second to merge then republishes the
# first's tag. The tag string never changed, so nothing signals that hosts
# pinned to it now resolve to a different image. The registry is the only thing
# that knows the serial was taken.
#
# Exits 0 when the tag is unused, 1 when it is already published, and 2 when it
# could not tell. A query that failed is not an absent tag: reporting one as the
# other would pass the very collision this exists to catch.

set -euo pipefail

image="${1:?usage: check-caddy-tag-unused.sh <image-ref-without-tag> <tag>}"
tag="${2:?usage: check-caddy-tag-unused.sh <image-ref-without-tag> <tag>}"

case "$image" in
  ghcr.io/*) repository="${image#ghcr.io/}" ;;
  *)
    echo "::error::this check only knows how to query ghcr.io, not \"$image\"."
    exit 2
    ;;
esac

# GHCR issues a pull token even for an anonymous caller on a public package, so
# this runs on a workstation as well as in CI. Credentials, where present, widen
# it to packages the caller can see but nobody else can.
auth=()
if [ -n "${GITHUB_TOKEN:-}" ]; then
  auth=(--user "${GITHUB_ACTOR:-x-access-token}:${GITHUB_TOKEN}")
fi

token_url="https://ghcr.io/token?service=ghcr.io&scope=repository:${repository}:pull"
if ! token_json="$(curl -fsSL --max-time 20 "${auth[@]}" "$token_url")"; then
  echo "::error::could not get a ghcr.io pull token for $repository, so whether \
$image:$tag is already published is unknown. Failing rather than assuming it is free."
  exit 2
fi

token="$(jq -r '.token // empty' <<<"$token_json")"
if [ -z "$token" ]; then
  echo "::error::ghcr.io returned no pull token for $repository, so whether \
$image:$tag is already published is unknown. Failing rather than assuming it is free."
  exit 2
fi

accept='application/vnd.oci.image.index.v1+json'
accept+=', application/vnd.docker.distribution.manifest.list.v2+json'
accept+=', application/vnd.oci.image.manifest.v1+json'
accept+=', application/vnd.docker.distribution.manifest.v2+json'

code="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 --head \
  -H "Authorization: Bearer $token" \
  -H "Accept: $accept" \
  "https://ghcr.io/v2/${repository}/manifests/${tag}" || echo 000)"

case "$code" in
  200)
    echo "::error::$image:$tag is already published. Another build claimed this \
serial, and publishing over it would move every host pinned to the tag onto a \
different image without the tag changing. Bump the serial again."
    exit 1
    ;;
  404)
    echo "$image:$tag is unused"
    exit 0
    ;;
  *)
    echo "::error::ghcr.io answered HTTP $code for $image:$tag, so whether it is \
already published is unknown. Failing rather than assuming it is free."
    exit 2
    ;;
esac
