#!/usr/bin/env bash
# Write the third-party notices Cyze ships with, to
# src-tauri/resources/THIRD-PARTY-NOTICES.txt (shown under Settings → About):
# the Rust crates in the app and in the bundled frostd server, the npm packages
# in the frontend, and the bundled cloudflared.
#
# Needs cargo-about (`cargo install cargo-about --features cli`) and the
# frontend's node_modules. Pass a frost-tools checkout to include frostd's
# crates; release builds always do.
#
#   scripts/third-party-notices.sh [path/to/frost-tools]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FROST_TOOLS="${1:-}"
OUT="$ROOT/src-tauri/resources/THIRD-PARTY-NOTICES.txt"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/src-tauri/Cargo.toml" | head -1)"
about() {
  cargo about generate --manifest-path "$1" -c "$ROOT/src-tauri/about.toml" "$ROOT/src-tauri/about.hbs"
}

mkdir -p "$(dirname "$OUT")"
{
  cat <<HEADER
Cyze $VERSION: third-party notices

Cyze is built on open-source software. This file lists it with the licenses
it's distributed under: the Rust crates in Cyze, the Rust crates in the frostd
server it bundles, the bundled cloudflared, and the JavaScript packages in its
interface. Cyze's own license is in the LICENSE file of its repository.

================================================================================
RUST CRATES IN CYZE
================================================================================
HEADER
  about "$ROOT/src-tauri/Cargo.toml"

  if [ -n "$FROST_TOOLS" ]; then
    cat <<HEADER

================================================================================
RUST CRATES IN THE BUNDLED FROSTD SERVER
(ZcashFoundation/frost-tools $(git -C "$FROST_TOOLS" rev-parse --short HEAD))
================================================================================
HEADER
    about "$FROST_TOOLS/frostd/Cargo.toml"
  fi

  cat <<HEADER

================================================================================
CLOUDFLARED
================================================================================
cloudflared $(tr -d '[:space:]' < "$ROOT/scripts/CLOUDFLARED_VERSION"), by Cloudflare, Inc., is bundled unmodified for the optional
Cloudflare quick tunnel. It is licensed under the Apache License 2.0 (its text
is reproduced above). Source: https://github.com/cloudflare/cloudflared

================================================================================
JAVASCRIPT PACKAGES IN THE INTERFACE
================================================================================
HEADER
  (cd "$ROOT" && node scripts/npm-notices.mjs)
} > "$OUT.tmp"
mv "$OUT.tmp" "$OUT"
echo "wrote $OUT ($(wc -l < "$OUT") lines)"
