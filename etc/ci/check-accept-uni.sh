#!/usr/bin/env bash
# Fail if a subscription-style request drives the handshake by hand.
#
# Reading the response envelope, classifying it, and only then awaiting the
# server-initiated data stream is one contract (i[stream.subscribe]) that four
# call sites used to re-implement, each getting a different part wrong: two
# blocked forever on an error response, one exited 0 on a rejection. It lives
# in OiClient::open_subscription now, and a fifth copy should not appear.
#
# The allowlist is for stream kinds whose framing genuinely differs: the shell
# protocol announces its uni stream IDs in the handshake, and the web gateway
# demuxes those streams for the browser.

set -euo pipefail

allowed=(
    'crates/protocol/src/client.rs'   # the shared helper itself
    'crates/ctl/src/shell.rs'         # i[stream.shell] — different framing
    'crates/web/src/daemon.rs'        # the shell uni-stream dispatcher
)

pattern='accept_uni'
mapfile -t hits < <(grep -rln --include='*.rs' "$pattern" crates/ | sort)

violations=()
for hit in "${hits[@]}"; do
    skip=false
    for ok in "${allowed[@]}"; do
        [[ "$hit" == "$ok" ]] && skip=true && break
    done
    $skip || violations+=("$hit")
done

if ((${#violations[@]})); then
    echo "error: accept_uni outside the allowlist:" >&2
    printf '  %s\n' "${violations[@]}" >&2
    cat >&2 <<'EOF'

Subscription-style requests must go through OiClient::open_subscription, which
reads and classifies the response envelope before awaiting the data stream. See
docs/logic-bug-audit-2026-07/theme-1-stream-error-handling.md. If this stream
kind really does frame differently (as the shell protocol does), add it to the
allowlist in this script with a comment saying why.
EOF
    exit 1
fi
