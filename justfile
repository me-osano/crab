# crab developer tasks. Run `just` for the default (check).

default: check

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    ./scripts/check-boundaries.sh

# Full stack: the default build is winit-only, so this proves the DRM/session/
# input backends still compile (mirrors the CI `clippy` job).
check-all:
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-features

build:
    cargo build --workspace

run:
    cargo run -- --backend winit

validate:
    cargo run -- --validate

boundaries:
    ./scripts/check-boundaries.sh
