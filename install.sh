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

# Otherwise, add it to the right shell profile(s) automatically (idempotent).
# Appends the line to $1 unless that file already references the bin dir.
append_path() {
    f="$1"
    line="$2"
    if [ -f "$f" ] && grep -qF "$BIN_DIR" "$f"; then
        return 1
    fi
    mkdir -p "$(dirname "$f")"
    printf '\n# added by sily installer\n%s\n' "$line" >> "$f"
    return 0
}

shell_name=$(basename "${SHELL:-sh}")
export_line="export PATH=\"$BIN_DIR:\$PATH\""
primary_rc=""

case "$shell_name" in
    fish)
        primary_rc="$HOME/.config/fish/config.fish"
        append_path "$primary_rc" "fish_add_path $BIN_DIR" || true
        ;;
    zsh)
        primary_rc="$HOME/.zshrc"
        append_path "$primary_rc" "$export_line" || true
        ;;
    bash)
        # Linux interactive shells read .bashrc; macOS login shells read
        # .bash_profile (falling back to .profile). Cover both.
        primary_rc="$HOME/.bashrc"
        append_path "$HOME/.bashrc" "$export_line" || true
        if [ -f "$HOME/.bash_profile" ]; then
            append_path "$HOME/.bash_profile" "$export_line" || true
        else
            append_path "$HOME/.profile" "$export_line" || true
        fi
        ;;
    *)
        primary_rc="$HOME/.profile"
        append_path "$primary_rc" "$export_line" || true
        ;;
esac

echo
echo "sily: added $BIN_DIR to your PATH ($shell_name profile)."
echo "      Use it now in this terminal:  source \"$primary_rc\""
echo "      (new terminals will have it automatically.)"
