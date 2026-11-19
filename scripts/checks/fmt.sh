#!/bin/sh
# Run `cargo fmt --all --check`. Standalone gate: no arguments.
set -eu
cd "$(git rev-parse --show-toplevel)"
echo "cargo fmt --all --check"
cargo fmt --all --check
