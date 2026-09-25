set shell := ["bash", "-euo", "pipefail", "-c"]

# List available recipes
default:
    @just --list

# Install the pinned toolchain and fetch dependencies
setup:
    mise install
    cargo fetch

# Format all code
fmt:
    cargo fmt --all

# Verify formatting without modifying files
fmt-check:
    cargo fmt --all -- --check

# Lint; warnings are errors.
# `web` is linted against wasm32, which is the only target it builds for.
lint:
    cargo clippy --workspace --exclude web --all-targets --all-features -- -D warnings
    cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings

# Run the test suite.
# `web` is excluded: it targets wasm32 and is exercised by `test-web`.
test:
    cargo test --workspace --exclude web --all-features
    cargo test -p web

# Build the web client bundle into crates/web/dist
build-web:
    cd crates/web && trunk build --release

# Every browser test, each against a freshly started server.
# Needs a Docker daemon, git and node; builds what it needs first.
test-e2e: build
    npm install --silent --no-fund --no-audit --no-save playwright @modelcontextprotocol/sdk @axe-core/playwright
    npx playwright install chromium
    tests/e2e/run.sh

# Mobile smoke path through a real browser.
# Needs a server running against a FRESH database, since it exercises
# first-run setup. Not part of `ci` yet: that needs browsers installed on the
# runner, which is packaging work.
#   GHOSTDOCK_URL=http://127.0.0.1:8080 just test-web
test-web:
    # Installed at the repo root: ES modules resolve node_modules by walking
    # up from the script, and ignore NODE_PATH entirely.
    npm install --silent --no-fund --no-audit --no-save playwright
    npx playwright install chromium
    node tests/e2e/smoke.mjs

# The deploy path end to end in a browser.
# Needs a running server with a FRESH database and a Docker daemon.
test-deploy:
    npm install --silent --no-fund --no-audit --no-save playwright
    npx playwright install chromium
    node tests/e2e/deploy.mjs

# Registering and deploying a Git-backed stack, in a browser.
# Needs a running server with a FRESH database, a Docker daemon, and
# GHOSTDOCK_TEST_REPO pointing at a git repository.
test-git:
    npm install --silent --no-fund --no-audit --no-save playwright
    npx playwright install chromium
    node tests/e2e/git.mjs

# A shell inside a container, in a browser.
# Needs a running server with a FRESH database, a Docker daemon, and one
# registered stack that is running.
test-shell:
    npm install --silent --no-fund --no-audit --no-save playwright
    npx playwright install chromium
    node tests/e2e/shell.mjs

# Performance budget for the web client, on a throttled phone profile.
# Needs a running server. Fails when a cold load stops being usable, which is
# the signal for revisiting the Wasm frontend decision.
test-perf:
    npm install --silent --no-fund --no-audit --no-save playwright
    npx playwright install chromium
    node tests/e2e/perf.mjs

# Enforce the invariant that `shared` builds for the browser.
# It is the contract between server and web client, so anything that
# sneaks a native-only dependency into it must fail here, not in M2.
check-wasm:
    cargo check -p shared --target wasm32-unknown-unknown

# Every scanner CI runs. Each covers something the others do not:
#   cargo deny  Rust advisories, licences, and where crates come from
#   gitleaks    secrets anywhere in history
#   hadolint    Dockerfile mistakes
#   actionlint  workflow syntax and embedded shell
#   zizmor      workflow security: injection, token scope, unpinned actions
#   trivy       vulnerable dependencies, secrets and misconfiguration together
security:
    cargo deny check
    gitleaks detect --no-banner --redact
    hadolint Dockerfile
    actionlint
    zizmor --offline .github/workflows
    # .env holds a developer's real credentials. It is gitignored, never
    # committed (gitleaks checks history) and absent in CI; trivy does not
    # read .gitignore, so it would otherwise fail every local run.
    trivy fs --quiet --ignorefile .trivyignore.yaml --skip-files .env --scanners vuln,secret,misconfig --severity HIGH,CRITICAL --exit-code 1 .

# Scans the built image, including the Debian packages the server shells
# out to. Separate from `security` because it needs an image to exist.
security-image image="ghostdock:local":
    trivy image --quiet --ignorefile .trivyignore.yaml --severity HIGH,CRITICAL --ignore-unfixed --exit-code 1 {{image}}

# Release build, server and web client
build: build-web
    cargo build --workspace --exclude web --release

# Run the server locally against ./.dev-data
run: build-web
    GHOSTDOCK_DATA_DIR=.dev-data \
    GHOSTDOCK_UI_DIR=crates/web/dist \
    GHOSTDOCK_BIND=127.0.0.1:8080 \
    cargo run --bin ghostdock

# Remove build artefacts
clean:
    cargo clean
    rm -rf crates/web/dist .dev-data

# Renders the PNG icons from icon.svg. Run after changing the SVG and
# commit the results. Needs rsvg-convert (dnf install librsvg2-tools).
icons:
    #!/usr/bin/env bash
    set -euo pipefail
    cd crates/web
    mkdir -p icons
    # Maskable and Apple icons are cropped to the platform's own shape, so
    # they get a full-bleed background instead of rounded corners.
    sed 's/rx="112"/rx="0"/' icon.svg > icons/.full-bleed.svg
    rsvg-convert -w 192 -h 192 icon.svg -o icons/icon-192.png
    rsvg-convert -w 512 -h 512 icon.svg -o icons/icon-512.png
    rsvg-convert -w 512 -h 512 icons/.full-bleed.svg -o icons/maskable-512.png
    rsvg-convert -w 180 -h 180 icons/.full-bleed.svg -o icons/apple-touch-icon.png
    rm icons/.full-bleed.svg

# Everything CI runs. CI calls this recipe and nothing else.
ci: fmt-check lint check-wasm test security build
