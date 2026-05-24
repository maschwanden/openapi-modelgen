set shell := ["bash", "-cu"]

# List available recipes
default:
  just --list

# Run the test suite
test:
    cargo test

# Check code formatting
check:
    cargo fmt --check

# Format all Rust code
fmt:
    cargo fmt --all

# Run clippy linter with warnings as errors
lint:
    cargo clippy -- -D warnings

# Build the project
build:
    cargo build

# Run the CLI (pass args after --)
run *args:
    cargo run -- {{args}}

# Run full CI pipeline
ci: check lint build test

