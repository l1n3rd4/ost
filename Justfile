# Teams CLI - Justfile

# List available recipes
default:
    @just --list

# --- Build ---

# Build teams-cli (debug)
build:
    cargo build

# Build teams-cli (release)
build-release:
    cargo build --release

# --- Quality ---

# Run clippy lints
lint:
    cargo clippy --all-targets --all-features

# Format code
fmt:
    cargo fmt

# Check formatting without changes
fmt-check:
    cargo fmt -- --check

# Run all quality checks
check: fmt-check lint
    cargo test --no-run

# --- Test ---

# Run unit tests
test:
    cargo test

# Run e2e tests (requires valid login session)
e2e: build
    ./tests/e2e_trouter.sh
    ./tests/e2e_read.sh
    ./tests/e2e_chats.sh
    ./tests/e2e_teams.sh

# --- Run ---

# Show CLI help
help:
    cargo run -- --help

# Login with device code flow
login:
    cargo run -- login

# Show authentication status
status:
    cargo run -- status

# Show current user info
whoami:
    cargo run -- whoami

# List recent chats
chats:
    cargo run -- chats

# List joined teams and channels
teams:
    cargo run -- teams

# Connect to Trouter for real-time notifications
trouter:
    cargo run -- trouter

# --- TUI ---

# Launch the terminal UI
tui: build
    ./target/debug/teams-cli tui
