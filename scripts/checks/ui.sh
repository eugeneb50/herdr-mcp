#!/bin/sh
# Run the web UI clippy gate for wasm32. Standalone gate: no arguments.
set -eu
cd "$(git rev-parse --show-toplevel)"
echo "cargo clippy -p herdr-mcp-ui --target wasm32-unknown-unknown -- -D warnings"
cargo clippy -p herdr-mcp-ui --target wasm32-unknown-unknown -- -D warnings
