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

# If the bin dir is already reachable, we're done.
case ":$PATH:" in
    *":$BIN_DIR:"*)
        echo "sily: ready — run 'sily list' to get started."
        exit 0
        ;;
esac

# Otherwise, add it to the right shell profile automatically (idempotent).
shell_name=$(basename "${SHELL:-sh}")
rc=""
added=0

case "$shell_name" in
    fish)
        rc="$HOME/.config/fish/config.fish"
        mkdir -p "$(dirname "$rc")"
        if ! { [ -f "$rc" ] && grep -qF "$BIN_DIR" "$rc"; }; then
            printf '\n# added by sily installer\nfish_add_path %s\n' "$BIN_DIR" >> "$rc"
            added=1
        fi
        ;;
    *)
        case "$shell_name" in
            zsh) rc="$HOME/.zshrc" ;;
            bash) rc="$HOME/.bashrc" ;;
            *) rc="$HOME/.profile" ;;
        esac
        if ! { [ -f "$rc" ] && grep -qF "$BIN_DIR" "$rc"; }; then
            printf '\n# added by sily installer\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$rc"
            added=1
        fi
        ;;
esac

echo
if [ "$added" -eq 1 ]; then
    echo "sily: added $BIN_DIR to your PATH in $rc"
    echo "      Run this now (or just open a new terminal) to use 'sily':"
    echo "        source \"$rc\""
else
    echo "sily: $BIN_DIR is configured in $rc but not active in this shell."
    echo "      Open a new terminal, or run:  source \"$rc\""
fi
