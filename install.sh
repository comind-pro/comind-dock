#!/bin/sh
# comind-dock installer:
#   curl -fsSL https://raw.githubusercontent.com/comind-pro/comind-dock/master/install.sh | sh
# Installs the latest GitHub release binary to ~/.local/bin (override with
# CDOCK_INSTALL_DIR). Verifies the sha256 when the release ships one.
set -eu

REPO="comind-pro/comind-dock"
BIN_DIR="${CDOCK_INSTALL_DIR:-$HOME/.local/bin}"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) echo "cdock: unsupported OS: $os" >&2; exit 1 ;;
esac
case "$arch" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64|amd64) arch=x86_64 ;;
  *) echo "cdock: unsupported arch: $arch" >&2; exit 1 ;;
esac
target="cdock-${arch}-${os}"

echo "cdock: resolving latest release for ${arch}-${os}..."
release_json=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")
url=$(printf '%s' "$release_json" \
  | grep '"browser_download_url"' | grep "$target.tar.gz\"" | head -1 | cut -d '"' -f 4)
if [ -z "$url" ]; then
  echo "cdock: no release asset $target.tar.gz found" >&2
  exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "cdock: downloading $url"
curl -fsSL "$url" -o "$tmp/cdock.tar.gz"

# Checksum: verify when the release publishes one.
sha_url=$(printf '%s' "$release_json" \
  | grep '"browser_download_url"' | grep "$target.tar.gz.sha256\"" | head -1 | cut -d '"' -f 4)
if [ -n "$sha_url" ]; then
  curl -fsSL "$sha_url" -o "$tmp/cdock.tar.gz.sha256"
  expected=$(cut -d ' ' -f 1 < "$tmp/cdock.tar.gz.sha256")
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/cdock.tar.gz" | cut -d ' ' -f 1)
  else
    actual=$(shasum -a 256 "$tmp/cdock.tar.gz" | cut -d ' ' -f 1)
  fi
  if [ "$expected" != "$actual" ]; then
    echo "cdock: checksum mismatch" >&2
    exit 1
  fi
  echo "cdock: checksum ok"
fi

tar -xzf "$tmp/cdock.tar.gz" -C "$tmp"
mkdir -p "$BIN_DIR"
mv "$tmp/cdock" "$BIN_DIR/cdock"
chmod +x "$BIN_DIR/cdock"

echo "cdock: installed $("$BIN_DIR/cdock" --version) to $BIN_DIR/cdock"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "cdock: note: $BIN_DIR is not in PATH — add: export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac
