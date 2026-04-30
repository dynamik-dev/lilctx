# https://just.systems — `cargo install just` then `just <recipe>`
set shell := ["bash", "-cu"]
export RUSTFLAGS := "-D warnings"

default: check

# Mirror CI locally: fmt, clippy, test, docs.
check: fmt clippy test docs

fmt:
    cargo fmt --all -- --check

fmt-fix:
    cargo fmt --all

clippy:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo test --all-features --workspace --locked

docs:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

deny:
    cargo deny check

audit:
    cargo audit

# Line coverage via cargo-llvm-cov. Resolves the toolchain's llvm-cov/llvm-profdata
# binaries because rustup's shim doesn't expose them on PATH.
coverage *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    bin_dir=$(ls -d ~/.rustup/toolchains/*/lib/rustlib/*/bin | head -1)
    export LLVM_COV="$bin_dir/llvm-cov"
    export LLVM_PROFDATA="$bin_dir/llvm-profdata"
    cargo llvm-cov --workspace {{ARGS}}

# One-shot install of the optional tools used by this repo.
install-tools:
    cargo install --locked cargo-deny cargo-audit cargo-llvm-cov just
