data_dir := "/opt/seedling"
verbosity := "-v"

# Build the workspace, skipping the frontend npm build
build:
    SKIP_FRONTEND_BUILD=1 cargo build

# Build with the frontend fully embedded (production)
build-release:
    cargo build --release

# Run cargo fmt
fmt:
    cargo fmt

# Run clippy and check formatting
check:
    cargo clippy && cargo fmt --check

# Run Rust tests (uses nextest if available, falls back to cargo test).
# Args filter the unit / integration pass; doc tests always run unfiltered
# since nextest does not execute them and target-selecting flags like --lib
# cannot be combined with --doc.
test *args:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo-nextest >/dev/null 2>&1; then
        cargo nextest run {{ args }}
    else
        cargo test --lib --bins --tests {{ args }}
    fi
    cargo test --doc

# Watch source files and rebuild on changes
watch-build:
    SKIP_FRONTEND_BUILD=1 watchexec -I cargo build

# Watch the built binary and restart the daemon on changes (requires sudo)
watch-run:
    rm seedling.log
    watchexec -IrW target/debug --ignore-nothing -f seedling \
        -E SSLKEYLOGFILE=/tmp/seedling.keylog \
        'sudo --preserve-env=SEEDLING_LOG --preserve-env=SSLKEYLOGFILE \
        target/debug/seedling --data-dir {{ data_dir }} {{ verbosity }} 2>&1 | tee -a seedling.log'

# Run seedling-ctl with arbitrary arguments
ctl *args:
    target/debug/seedling-ctl {{ args }}

# Tail the live event feed from the daemon
events:
    target/debug/seedling-ctl op events

# Watch the built seedling-web binary and restart it on changes
watch-web:
    watchexec -IrW target/debug --ignore-nothing -f seedling-web \
        -E SSLKEYLOGFILE=/tmp/seedling-web.keylog \
        'target/debug/seedling-web --dev-no-auth'

# Run the Vite dev server
frontend:
    cd crates/web/frontend && npm run dev

# Build the frontend bundle (also runs automatically via build.rs on cargo build)
frontend-build:
    cd crates/web/frontend && npm run build

# Install frontend npm dependencies
frontend-install:
    cd crates/web/frontend && npm install

# Run frontend unit tests (vitest)
frontend-test:
    cd crates/web/frontend && npm test

# Watch frontend unit tests
frontend-test-watch:
    cd crates/web/frontend && npm run test:watch

# Run the OWASP ZAP baseline (passive) scan against a stubbed stack (needs docker)
dast: build
    #!/usr/bin/env bash
    set -euo pipefail
    work="$(mktemp -d)"
    # Leave the report behind for inspection; only stop the spawned processes.
    trap 'kill $(cat "$work/pids" 2>/dev/null) 2>/dev/null || true' EXIT
    etc/ci/dast-stack.sh "$work"
    port="$(cat "$work/http-port")"
    cp .zap/rules.tsv "$work/rules.tsv"
    docker run --rm --network=host -v "$work:/zap/wrk:rw" \
        ghcr.io/zaproxy/zaproxy:stable \
        zap-baseline.py -t "http://127.0.0.1:$port" -c rules.tsv -r report.html
    echo "report: $work/report.html"

# Run Playwright end-to-end tests (spawns a stubbed daemon + web pair)
frontend-e2e: build
    cd crates/web/frontend && npm run test:e2e

# Run Playwright e2e tests with the interactive UI runner
frontend-e2e-ui: build
    cd crates/web/frontend && npm run test:e2e:ui
