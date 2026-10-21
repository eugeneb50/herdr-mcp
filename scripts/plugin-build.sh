#!/bin/sh
# herdr-plugin.toml's [[build]]: herdr runs this from the plugin checkout on
# `herdr plugin install`. Bundles the web UI when `dx` is installed (the
# build then compiles it into herdr-mcp) and puts herdr-mcp in ./bin (the
# command the manifest runs).
set -eu
# Build into ./target whatever CARGO_TARGET_DIR says: the copies below and the
# bundle herdr-mcp's build picks up are paths in this checkout.
export CARGO_TARGET_DIR="$PWD/target"

if command -v dx >/dev/null 2>&1; then
    (cd crates/herdr-mcp-ui && dx bundle --platform web --release)
else
    echo "dx not found: no web UI (cargo install dioxus-cli, then reinstall)" >&2
fi

cargo build --release --locked -p herdr-mcp-cli --bin herdr-mcp
mkdir -p bin
# Copy, then rename over: a running bin/herdr-mcp (the bridge tab, the
# dashboard) makes a plain cp fail with "Text file busy".
cp target/release/herdr-mcp bin/herdr-mcp.new
mv bin/herdr-mcp.new bin/herdr-mcp
