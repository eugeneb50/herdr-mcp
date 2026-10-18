#!/bin/sh
# herdr-plugin.toml's [[build]]: herdr runs this from the plugin checkout on
# `herdr plugin install`. Puts herdr-mcp and herdr-trim in ./bin (the commands
# the manifest runs) and bundles the web UI when `dx` is installed.
set -eu

cargo build --release -p herdr-mcp-cli --bin herdr-mcp --bin herdr-trim
mkdir -p bin
cp target/release/herdr-mcp target/release/herdr-trim bin/

if command -v dx >/dev/null 2>&1; then
    (cd crates/herdr-mcp-ui && dx bundle --platform web --release)
else
    echo "dx not found: no web UI (cargo install dioxus-cli, then reinstall)" >&2
fi
