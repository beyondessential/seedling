#!/usr/bin/env bash
# Print every citable target in the specs: `ID` for each spec and `ID#slug`
# for each heading in it, slugged as GitHub renders heading anchors.

set -euo pipefail

root="${1:-.workhorse/specs}"

find "$root" -name '*.md' -print0 | sort -z | while IFS= read -r -d '' file; do
    id="$(awk 'NR == 1 && $0 != "---" { exit } NR > 1 && /^---$/ { exit } /^id: / { print $2; exit }' "$file")"
    [[ -n "$id" ]] || continue
    printf '%s\t%s\n' "$id" "$file"
    awk '/^```/ { fence = !fence } !fence && /^#{1,6} / { sub(/^#+ +/, ""); print }' "$file" |
        tr '[:upper:]' '[:lower:]' |
        sed -E 's/[^a-z0-9 _-]//g; s/ /-/g' |
        while IFS= read -r slug; do
            printf '%s#%s\t%s\n' "$id" "$slug" "$file"
        done
done
