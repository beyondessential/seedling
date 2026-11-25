#!/usr/bin/env bash
# Check every definition under apps/ with `seedling-ctl definitions check`,
# reporting each that fails rather than stopping at the first.

set -euo pipefail

ctl="${1:-target/debug/seedling-ctl}"
failed=0

for def in apps/*; do
    args=(--const AVAILABLE_MEMORY=2147483648)
    case "$def" in
        apps/postgres.seed.rhai)
            args+=(--set version=18.3 --set password=correct-horse-battery-staple-9)
            ;;
    esac
    "$ctl" definitions check "$def" "${args[@]}" || failed=1
done

exit "$failed"
