#!/usr/bin/env bash
# NovaDB CI-ready check (Linux/macOS)
# Runs the exact gate used by CI: format, clippy, then the full test suite.
set -euo pipefail

echo "==> cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "==> cargo clippy --workspace --all-targets --all-features -- -D warnings"
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "==> cargo test --workspace"
cargo test --workspace

echo "ALL CHECKS PASSED"