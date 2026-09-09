#!/usr/bin/env bash
# Install the latest acts-server and acts-cli release binaries on macOS/Linux.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/yaojianpin/acts/main/install.sh | bash
#   ACTS_VERSION=v0.24.0 curl -fsSL https://raw.githubusercontent.com/yaojianpin/acts/main/install.sh | bash
#
# The binaries land in ~/.acts/bin (override with ACTS_INSTALL_DIR).

set -euo pipefail

REPO="${ACTS_REPO:-yaojianpin/acts}"
VERSION="${ACTS_VERSION:-latest}"
INSTALL_DIR="${ACTS_INSTALL_DIR:-$HOME/.acts/bin}"

log() { printf '\033[32m%s\033[0m\n' "$*"; }
err() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Linux) os="linux" ;;
  Darwin) os="macos" ;;
  *) err "unsupported OS '$(uname -s)'; install.sh covers macOS and Linux, use install.ps1 on Windows" ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) err "unsupported architecture '$(uname -m)'" ;;
esac

if [ "$VERSION" = "latest" ]; then
  log "resolving the latest release of $REPO"
  tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)"
  [ -n "$tag" ] || err "failed to resolve the latest release of $REPO"
else
  tag="$VERSION"
fi
version="${tag#v}"

asset="acts-${version}-${os}-${arch}.tar.gz"
url="https://github.com/$REPO/releases/download/${tag}/${asset}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

log "downloading $url"
curl -fsSL "$url" -o "$tmp/acts.tar.gz" || err "download failed: $url"
tar -xzf "$tmp/acts.tar.gz" -C "$tmp"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/acts-server" "$tmp/acts-cli" "$INSTALL_DIR/"

log "installed acts-server ${version} and acts-cli ${version} to $INSTALL_DIR"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) log "add $INSTALL_DIR to your PATH, e.g.: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
