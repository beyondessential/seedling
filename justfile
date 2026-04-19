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

# Watch source files and rebuild on changes
watch-build:
    SKIP_FRONTEND_BUILD=1 watchexec cargo build

# Watch the built binary and restart the daemon on changes (requires sudo)
watch-run:
    watchexec -IrW target/debug --ignore-nothing \
        -E SSLKEYLOGFILE=/tmp/seedling.keylog \
        'sudo --preserve-env=SEEDLING_LOG --preserve-env=SSLKEYLOGFILE \
        target/debug/seedling --data-dir {{data_dir}} {{verbosity}} 2>&1 | tee -a seedling.log'

# Run seedling-ctl with arbitrary arguments
ctl *args:
    target/debug/seedling-ctl {{args}}

# Tail the live event feed from the daemon
events:
    target/debug/seedling-ctl op events

# Watch the built seedling-web binary and restart it on changes
watch-web:
    watchexec -IrW target/debug --ignore-nothing -f seedling-web \
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
