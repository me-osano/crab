# crawm developer tasks. Run `just` for the default (check).

default: check

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    ./scripts/check-boundaries.sh

build:
    cargo build --workspace

run:
    cargo run -p crawm -- --backend winit

validate:
    cargo run -p crawm -- --validate

boundaries:
    ./scripts/check-boundaries.sh
