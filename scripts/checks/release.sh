#!/bin/sh
# Build the release binary of the CLI. Standalone gate: no arguments.
set -eu
cd "$(git rev-parse --show-toplevel)"
echo "cargo build --release -p herdr-mcp-cli"
cargo build --release -p herdr-mcp-cli
