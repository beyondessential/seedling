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

# GHCR issues a pull token even to an anonymous caller for a public package, so
# this runs on a workstation as well as in CI. Credentials, where present, widen
# that to a package only the caller can see; a pull request from a fork gets a
# read-only token, which may be refused the exchange, so a refusal falls back to
# anonymous rather than failing a check the public package does not need it for.
token_url="https://ghcr.io/token?service=ghcr.io&scope=repository:${repository}:pull"

# Answers with the token, or with nothing if this caller cannot have one. A
# refusal is not this function's to report: the caller has another way to ask.
fetch_token() {
  local json
  json="$(curl -fsSL --max-time 20 "$@" "$token_url" 2>/dev/null)" || return 0
  jq -r '.token // empty' <<<"$json"
}

token=""
if [ -n "${GITHUB_TOKEN:-}" ]; then
  token="$(fetch_token --user "${GITHUB_ACTOR:-x-access-token}:${GITHUB_TOKEN}")"
fi
if [ -z "$token" ]; then
  token="$(fetch_token)"
fi

if [ -z "$token" ]; then
  echo "::error::could not get a ghcr.io pull token for $repository, so whether \
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
