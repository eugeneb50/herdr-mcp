#!/bin/sh
# Run the server-side clippy gate (native crates). Standalone gate: no arguments.
set -eu
cd "$(git rev-parse --show-toplevel)"
echo "cargo clippy --workspace --exclude herdr-mcp-ui --all-targets -- -D warnings"
cargo clippy --workspace --exclude herdr-mcp-ui --all-targets -- -D warnings
