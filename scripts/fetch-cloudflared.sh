#!/usr/bin/env bash
# Download the pinned cloudflared release for a target triple, verify its
# SHA-256, and install the binary at DEST.
#
#   scripts/fetch-cloudflared.sh <target-triple> <dest>
#
# The release is pinned in scripts/CLOUDFLARED_VERSION and each asset's hash in
# scripts/cloudflared.sha256, so every build bundles the same verified binary.
# A download that doesn't match its hash fails the build.
#
# To update: pick a release at least two weeks old, put its tag in
# CLOUDFLARED_VERSION, and record the SHA-256 of each asset as downloaded. The
# GitHub release page lists it as the asset's digest. For the macOS .tgz files
# that is the hash of the archive; the release notes list the binary inside it,
# which is a different value.
set -euo pipefail

if [ $# -ne 2 ]; then
  echo "usage: $0 <target-triple> <dest>" >&2
  exit 2
fi
TRIPLE="$1"
DEST="$2"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION="$(tr -d '[:space:]' < "$SCRIPT_DIR/CLOUDFLARED_VERSION")"

case "$TRIPLE" in
  x86_64-unknown-linux-gnu)   ASSET=cloudflared-linux-amd64       ;;
  aarch64-unknown-linux-gnu)  ASSET=cloudflared-linux-arm64       ;;
  x86_64-pc-windows-msvc)     ASSET=cloudflared-windows-amd64.exe ;;
  aarch64-apple-darwin)       ASSET=cloudflared-darwin-arm64.tgz  ;;
  x86_64-apple-darwin)        ASSET=cloudflared-darwin-amd64.tgz  ;;
  *)
    echo "error: no cloudflared asset known for '$TRIPLE'." >&2
    exit 1
    ;;
esac

EXPECTED="$(awk -v a="$ASSET" '$2 == a { print $1 }' "$SCRIPT_DIR/cloudflared.sha256")"
if [ -z "$EXPECTED" ]; then
  echo "error: no pinned SHA-256 for $ASSET in scripts/cloudflared.sha256" >&2
  exit 1
fi

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
URL="https://github.com/cloudflare/cloudflared/releases/download/$VERSION/$ASSET"
echo "fetching cloudflared $VERSION ($ASSET)…"
curl -fsSL --retry 3 "$URL" -o "$TMP/$ASSET"

ACTUAL="$(sha256_of "$TMP/$ASSET")"
if [ "$ACTUAL" != "$EXPECTED" ]; then
  echo "error: SHA-256 mismatch for $ASSET" >&2
  echo "  expected $EXPECTED" >&2
  echo "  got      $ACTUAL" >&2
  exit 1
fi

mkdir -p "$(dirname "$DEST")"
case "$ASSET" in
  *.tgz) tar -xzf "$TMP/$ASSET" -C "$TMP"; cp "$TMP/cloudflared" "$DEST" ;;
  *)     cp "$TMP/$ASSET" "$DEST" ;;
esac
chmod +x "$DEST" || true
echo "cloudflared $VERSION verified and installed at $DEST"
