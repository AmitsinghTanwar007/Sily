#!/bin/sh
# sily installer — downloads the latest prebuilt binary for your platform.
#   curl -fsSL https://raw.githubusercontent.com/AmitsinghTanwar007/Sily/main/install.sh | sh
#
# Override the install location with SILY_BIN_DIR (default: ~/.local/bin).
set -eu

REPO="AmitsinghTanwar007/Sily"
BIN_DIR="${SILY_BIN_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)

case "$os" in
    Linux) os_name=linux ;;
    Darwin) os_name=macos ;;
    *) echo "sily: unsupported OS '$os'" >&2; exit 1 ;;
esac

case "$arch" in
    x86_64 | amd64) arch_name=x86_64 ;;
    arm64 | aarch64) arch_name=arm64 ;;
    *) echo "sily: unsupported architecture '$arch'" >&2; exit 1 ;;
esac

asset="sily-${os_name}-${arch_name}.tar.gz"
url="https://github.com/${REPO}/releases/latest/download/${asset}"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "sily: downloading ${asset} ..."
if ! curl -fsSL "$url" -o "$tmp/$asset"; then
    echo "sily: download failed from $url" >&2
    echo "      (has a release been published yet?)" >&2
    exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$BIN_DIR"
install -m 0755 "$tmp/sily" "$BIN_DIR/sily"

echo "sily: installed to $BIN_DIR/sily"
"$BIN_DIR/sily" --version || true

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo
        echo "Add $BIN_DIR to your PATH to run 'sily' directly:"
        echo "  export PATH=\"$BIN_DIR:\$PATH\""
        ;;
esac
