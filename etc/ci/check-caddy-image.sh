#!/usr/bin/env bash
# Check a built seedling-caddy image against what the runtime requires of it.
#
# Two hard checks and one advisory:
#
#   - Every module in docker/caddy/required-modules.txt is registered by the
#     image. Caddy rejects a whole configuration that names a module it does
#     not provide, so a missing module is not a degraded route: it is an
#     unappliable configuration, which aborts the proxy upgrade and leaves the
#     previous config serving indefinitely.
#
#   - The image's Caddy version matches the version docker/caddy/Containerfile
#     pins. The pin is implicit (the builder image sets CADDY_VERSION and xcaddy
#     reads it), so this is what catches a base-image change that moves it.
#
#   - The final stage does not derive from an image that already ships a Caddy.
#     Layering our binary over the official image leaves its ~48 MB Caddy in a
#     lower layer, still pulled by every host, for a binary nothing runs.
#
#   - The Caddy version matches the one beyondessential/third-party-builds
#     builds for the ad-hoc hosts. This WARNS rather than fails. Parity keeps a
#     host that is mid-migration on a single Caddy version, but Seedling
#     controls its own Caddy version and must not have its image build broken by
#     another repository's bump. A check, not a dependency.

set -euo pipefail

image="${1:?usage: check-caddy-image.sh <image-ref>}"
# CI has docker; a workstation is more likely to have podman.
runtime="${CONTAINER_RUNTIME:-docker}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
required="$root/docker/caddy/required-modules.txt"
containerfile="$root/docker/caddy/Containerfile"

fail=0

# --- modules ---------------------------------------------------------------

modules="$("$runtime" run --rm "$image" caddy list-modules)"

missing=()
while read -r module; do
  grep -qxF -- "$module" <<<"$modules" || missing+=("$module")
done < <(sed -e 's/#.*//' -e 's/[[:space:]]//g' "$required" | grep -v '^$')

if [ ${#missing[@]} -ne 0 ]; then
  echo "::error::image is missing Caddy modules the runtime requires: ${missing[*]}"
  echo "A configuration naming any of these is rejected whole, not partially."
  fail=1
else
  echo "all $(sed -e 's/#.*//' -e '/^[[:space:]]*$/d' "$required" | wc -l) required modules present"
fi

# --- Caddy version matches the Containerfile pin ---------------------------

expected="$(sed -n 's|^FROM docker\.io/library/caddy:\([0-9][0-9.]*\)-builder.*|\1|p' "$containerfile" | head -n1)"
actual="$("$runtime" run --rm "$image" caddy version | head -n1 | awk '{print $1}')"
actual="${actual#v}"

if [ -z "$expected" ]; then
  echo "::error::could not read the pinned Caddy version from $containerfile"
  fail=1
elif [ "$expected" != "$actual" ]; then
  echo "::error::image reports Caddy $actual but the Containerfile pins $expected"
  fail=1
else
  echo "Caddy version $actual matches the Containerfile pin"
fi

# --- one Caddy binary in the image -----------------------------------------

final_from="$(grep -E '^FROM ' "$containerfile" | tail -n1)"
if grep -qE '^FROM .*/caddy:' <<<"$final_from"; then
  echo "::error::the final stage derives from a Caddy image ($final_from), so the \
image ships two Caddy binaries and hosts pull both."
  fail=1
else
  echo "final stage carries one Caddy binary"
fi

# --- advisory: parity with the ad-hoc hosts' build --------------------------

bes_workflow="https://raw.githubusercontent.com/beyondessential/third-party-builds/main/.github/workflows/caddy.yml"
bes="$(curl -fsSL --max-time 20 "$bes_workflow" 2>/dev/null \
  | sed -n 's/^[[:space:]]*CADDY_VERSION:[[:space:]]*//p' | tr -d "\"' " | head -n1 || true)"

if [ -z "$bes" ]; then
  echo "note: could not read the third-party-builds Caddy version; skipping the parity check"
elif [ "$bes" != "$actual" ]; then
  echo "::warning::Caddy $actual here vs $bes in third-party-builds. A host that is \
mid-migration then runs two Caddy versions at once. Deliberate is fine; unnoticed is not."
else
  echo "Caddy version matches third-party-builds ($bes)"
fi

exit "$fail"
