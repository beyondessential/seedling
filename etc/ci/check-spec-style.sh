#!/usr/bin/env bash
# Fail on spec files that break the mechanical parts of the house style in
# .workhorse/rules.md: frontmatter ids, heading anchors, em-dashes, and
# requirement keywords inside non-normative notes.

set -euo pipefail

root="${1:-.workhorse/specs}"
here="$(dirname "$0")"
problems=()

while IFS= read -r -d '' file; do
    id="$(awk 'NR == 1 && $0 != "---" { exit } NR > 1 && /^---$/ { exit } /^id: / { print $2; exit }' "$file")"
    if [[ -z "$id" ]]; then
        problems+=("$file: no frontmatter id")
    elif [[ ! "$id" =~ ^[A-Z][A-Z0-9]{1,5}$ ]]; then
        problems+=("$file: id '$id' is not 2-6 uppercase letters and digits")
    fi

    while IFS= read -r hit; do
        problems+=("$file:$hit: em-dash")
    done < <(grep -n '—' "$file" | cut -d: -f1 || true)

    # A keyword inside a note gives non-normative text normative force.
    while IFS= read -r hit; do
        problems+=("$file:$hit")
    done < <(awk '
        /^> \[!NOTE\]/ { note = 1; next }
        note && !/^>/ { note = 0 }
        note && /(^|[^A-Za-z])(MUST|SHALL|SHOULD|REQUIRED|RECOMMENDED|OPTIONAL|MAY)([^A-Za-z]|$)/ {
            print NR ": requirement keyword inside a note"
        }
    ' "$file")
done < <(find "$root" -name '*.md' -print0 | sort -z)

while IFS= read -r dup; do
    problems+=("duplicate spec id or heading anchor: $dup")
done < <("$here/spec-slugs.sh" "$root" | cut -f1 | sort | uniq -d)

if ((${#problems[@]})); then
    echo "error: spec style problems:" >&2
    printf '  %s\n' "${problems[@]}" >&2
    exit 1
fi
