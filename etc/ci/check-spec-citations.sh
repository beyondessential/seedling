#!/usr/bin/env bash
# Fail on a `spec: ID` or `spec: ID#slug` citation in code that names no spec
# or no heading in it.
#
# Code cites the requirement it implements by the id in a spec's frontmatter
# and, optionally, the anchor of the section holding the requirement. Anchors
# come from headings, so rewording a heading silently orphans every citation
# of it; this check turns that into a failure in the same change.

set -euo pipefail

here="$(dirname "$0")"
targets="$(mktemp)"
trap 'rm -f "$targets"' EXIT
"$here/spec-slugs.sh" | cut -f1 | sort -u > "$targets"

violations=()
while IFS= read -r hit; do
    file="${hit%%:*}"
    rest="${hit#*:}"
    line="${rest%%:*}"
    text="${rest#*:}"
    while IFS= read -r cite; do
        grep -qxF -- "$cite" "$targets" || violations+=("$file:$line: $cite")
    done < <(grep -oE 'spec: [A-Z][A-Z0-9]+(#[a-z0-9_-]+)?(, [A-Z][A-Z0-9]+(#[a-z0-9_-]+)?)*' <<<"$text" |
        sed -E 's/^spec: //' | tr ',' '\n' | sed -E 's/^ +//')
done < <(grep -rnE --include='*.rs' --include='*.ts' --include='*.tsx' --include='*.sh' \
    --exclude-dir=target --exclude-dir=node_modules --exclude-dir=dist \
    'spec: [A-Z][A-Z0-9]+' crates/ etc/ || true)

if ((${#violations[@]})); then
    echo "error: spec citations that resolve to no spec or heading:" >&2
    printf '  %s\n' "${violations[@]}" >&2
    cat >&2 <<'EOT'

A citation is `spec: ID` or `spec: ID#slug`, where ID is a spec's frontmatter
id under .workhorse/specs/ and slug is the anchor of one of its headings. If a
heading was reworded, update the citations to the new anchor.
EOT
    exit 1
fi
