#!/usr/bin/env bash
# Check every demo under demos/ with `seedling-ctl definitions check`,
# reporting each that fails rather than stopping at the first.

set -euo pipefail

ctl="${1:-target/debug/seedling-ctl}"
failed=0

for def in demos/*/; do
    def="${def%/}"
    args=()
    case "$def" in
        demos/shared)
            continue
            ;;
        demos/params-tour)
            args+=(--set contact=ops@example.com)
            ;;
        demos/workloads-tour)
            args+=(--const AVAILABLE_MEMORY=2147483648)
            ;;
    esac
    "$ctl" definitions check "$def" ${args[@]+"${args[@]}"} || failed=1
done

exit "$failed"
