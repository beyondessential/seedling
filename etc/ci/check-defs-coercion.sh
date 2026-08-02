#!/usr/bin/env bash
# Fail if the BSL defs layer consumes a script value without a throwing
# conversion.
#
# `into_string().unwrap_or_default()`, `filter_map(try_cast)` and bare `as`
# casts do not fail on a script type error — they change what the script
# means. A dropped `select` criterion makes the selection match every resource
# in the app; a wrapped `pids_limit` becomes 1; a non-string argv element
# becomes "". Each surfaces far from the cause, at reconcile or container-start
# time, with no pointer back to the script line.
#
# crates/core/src/defs/take.rs is the throwing alternative. Genuine type
# dispatch — the successive casts in `col()`, the string-vs-array dispatch in
# `take_command_cmd` — is exempt when it ends in a throw; mark such a site with
# a `// take: dispatch` comment on the line or the one above it.

set -euo pipefail

scope='crates/core/src/defs'
exempt_file='crates/core/src/defs/take.rs'

# The idioms, as they appear textually. `as u16`/`as u32` are included because
# an unchecked narrowing cast of a script-supplied i64 is the same bug wearing
# different syntax.
pattern='into_string\(\)\.unwrap_or_default\(\)|into_string\(\)\.ok\(\)|as_bool\(\)\.ok\(\)|as_int\(\)\.ok\(\)|filter_map\(.*try_cast|[^_a-zA-Z] as u(8|16|32)\b'

violations=()
while IFS= read -r hit; do
    file="${hit%%:*}"
    rest="${hit#*:}"
    line="${rest%%:*}"
    [[ "$file" == "$exempt_file" ]] && continue
    # `// take: dispatch` on this line or the previous one exempts the site.
    context="$(sed -n "$((line > 1 ? line - 1 : 1)),${line}p" "$file")"
    [[ "$context" == *"take: dispatch"* ]] && continue
    violations+=("$hit")
done < <(grep -rnE --include='*.rs' "$pattern" "$scope" || true)

if ((${#violations[@]})); then
    echo "error: silent coercion of a script value in the defs layer:" >&2
    printf '  %s\n' "${violations[@]}" >&2
    cat >&2 <<'EOF'

Use the throwing conversions in crates/core/src/defs/take.rs, which name the
argument and the actual type so the script author sees the line that is wrong.
See docs/logic-bug-audit-2026-07/theme-6-bsl-strict-validation.md. If this site
is genuine type dispatch that ends in a throw, mark it `// take: dispatch`.
EOF
    exit 1
fi
