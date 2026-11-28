# Sourced by the entry points. Recipe steps run with a HOME of the rig's own,
# so point rustup and cargo at the person's toolchain and cargo home (the
# registry cache and any compiler wrapper configured there) unless the person
# set them explicitly.
set -eu

if [ -z "${RUSTUP_HOME:-}" ] && [ -n "${RIG_PERSON_HOME:-}" ] && [ -d "$RIG_PERSON_HOME/.rustup" ]; then
    export RUSTUP_HOME="$RIG_PERSON_HOME/.rustup"
fi
if [ -z "${CARGO_HOME:-}" ] && [ -n "${RIG_PERSON_HOME:-}" ] && [ -d "$RIG_PERSON_HOME/.cargo" ]; then
    export CARGO_HOME="$RIG_PERSON_HOME/.cargo"
fi

FRONTEND="$RIG_SOURCE/crates/web/frontend"
BIN="$RIG_DATA_DIR/bin"
