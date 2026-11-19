#!/bin/sh
# Run the workspace test suite (excludes the web UI crate). Standalone gate: no arguments.
set -eu
cd "$(git rev-parse --show-toplevel)"
echo "cargo nextest run --workspace --exclude herdr-mcp-ui"
cargo nextest run --workspace --exclude herdr-mcp-ui
