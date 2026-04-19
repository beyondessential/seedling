vite_port := "5173"

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

# Run seedling-web in dev mode, proxying the SPA to the Vite dev server
web:
    SKIP_FRONTEND_BUILD=1 cargo run -p seedling-web -- --dev-no-auth --vite-port {{vite_port}}

# Run the Vite dev server
frontend:
    cd crates/web/frontend && npm run dev

# Build the frontend bundle (also runs automatically via build.rs on cargo build)
frontend-build:
    cd crates/web/frontend && npm run build

# Install frontend npm dependencies
frontend-install:
    cd crates/web/frontend && npm install
