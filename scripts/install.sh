#!/bin/sh
# Installs herdr-mcp from the latest GitHub release:
#   curl -fsSL https://raw.githubusercontent.com/eugeneb50/herdr-mcp/main/scripts/install.sh | sh
# HERDR_MCP_INSTALL_DIR picks the directory (default ~/.local/bin).
set -eu

repo=eugeneb50/herdr-mcp
dir=${HERDR_MCP_INSTALL_DIR:-$HOME/.local/bin}

case "$(uname -s)-$(uname -m)" in
    # Static musl builds: no glibc version requirement.
    Linux-x86_64) target=x86_64-unknown-linux-musl ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Darwin-x86_64) target=x86_64-apple-darwin ;;
    *)
        echo "herdr-mcp: no release build for $(uname -s) $(uname -m); build from source" >&2
        exit 1
        ;;
esac

name=herdr-mcp-$target
url=https://github.com/$repo/releases/latest/download/$name.tar.gz
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "downloading $url"
curl -fsSL "$url" -o "$tmp/$name.tar.gz"
curl -fsSL "$url.sha256" -o "$tmp/$name.tar.gz.sha256"
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$tmp" && sha256sum -c "$name.tar.gz.sha256" >/dev/null)
else
    (cd "$tmp" && shasum -a 256 -c "$name.tar.gz.sha256" >/dev/null)
fi

tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$dir"
install -m 755 "$tmp/$name/herdr-mcp" "$dir/herdr-mcp"
echo "installed herdr-mcp in $dir"

case ":$PATH:" in
    *":$dir:"*) ;;
    *) echo "add $dir to PATH to run it" ;;
esac
