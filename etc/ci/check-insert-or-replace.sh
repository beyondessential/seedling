#!/usr/bin/env bash
# Fail on new `INSERT OR REPLACE` outside the allowlist.
#
# INSERT OR REPLACE is delete-then-insert: every column the statement does not
# name reverts to its default. That is safe only while one writer owns the
# whole row — and it stops being safe the moment a migration adds a column
# another writer owns, which is precisely how `current_operation` came to
# silently drop a persisted cancellation. The failure arrives with the
# migration, not with the statement, so it cannot be caught by reviewing the
# statement alone.
#
# Prefer `INSERT ... ON CONFLICT(key) DO UPDATE SET col = excluded.col, ...`,
# which names what it writes and leaves the rest alone.
#
# Migrations are exempt: they run once against a known schema.

set -euo pipefail

# Each entry is a file that may use it, with the reason.
#   history.rs   — action_log's positional overwrite keyed on
#                  (operation_id, call_index) IS the replay contract: rewriting
#                  a call's entry in place is the intended semantics.
allowed=(
    'crates/core/src/runtime/history.rs'
)

violations=()
while IFS= read -r hit; do
    file="${hit%%:*}"
    [[ "$file" == *"/migrations/"* ]] && continue
    # Prose mentioning the statement (comments, test assertions) is not a use.
    line="${hit#*:}"
    line="${line#*:}"
    [[ "$line" =~ (//|--|\") ]] && [[ ! "$line" =~ INSERT\ OR\ REPLACE\ INTO ]] && continue
    skip=false
    for ok in "${allowed[@]}"; do
        [[ "$file" == "$ok" ]] && skip=true && break
    done
    $skip || violations+=("$hit")
done < <(grep -rn --include='*.rs' 'INSERT OR REPLACE' crates/ || true)

if ((${#violations[@]})); then
    echo "error: INSERT OR REPLACE outside the allowlist:" >&2
    printf '  %s\n' "${violations[@]}" >&2
    cat >&2 <<'EOF'

INSERT OR REPLACE resets every column the statement does not name, so it is
only safe while a single writer owns the whole row — and an ALTER TABLE ADD
COLUMN can end that at any time, silently. Use
`INSERT ... ON CONFLICT(key) DO UPDATE SET ...` naming the columns this writer
owns. See docs/logic-bug-audit-2026-07/theme-8-restart-replay.md.
EOF
    exit 1
fi
