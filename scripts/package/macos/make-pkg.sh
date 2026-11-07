#!/usr/bin/env bash
set -euo pipefail

# Build a simple unsigned .pkg containing the grindvakt binary and share files.
# No auto-load or launchd install occurs here.

ROOT_DIR="$(cd "$(dirname "$0")"/../../.. && pwd)"
STAGE_DIR="$(mktemp -d)"
cleanup() { rm -rf "$STAGE_DIR"; }
trap cleanup EXIT
PKG_ID="com.weirdbrains.grindvakt"
VERSION="$(awk -F\\\" '/^version *=/ {print $2; exit}' "$ROOT_DIR/Cargo.toml")"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
BIN_PATH="$CARGO_TARGET_DIR/release/grindvakt"
OUT_PKG="$CARGO_TARGET_DIR/grindvakt-$VERSION-macos.pkg"

echo "Staging payload under $STAGE_DIR"
install -d "$STAGE_DIR/usr/local/bin"
install -m 0755 "$BIN_PATH" "$STAGE_DIR/usr/local/bin/grindvakt"

install -d "$STAGE_DIR/usr/local/share/grindvakt"
cat > "$STAGE_DIR/usr/local/share/grindvakt/README-macos.txt" <<'EOF'
Grindvakt macOS notes:
- Install a per-user launch agent with:
    grindvakt service install
- Print the agent first with:
    grindvakt service print
- Enroll unattended Keychain credential (DEK) with:
    grindvakt service enroll-credential
- The installer does not auto-load the agent and preserves your vault.
EOF

echo "Building pkg at $OUT_PKG"
pkgbuild \
  --install-location / \
  --root "$STAGE_DIR" \
  --identifier "$PKG_ID" \
  --version "$VERSION" \
  "$OUT_PKG"

echo "Done: $OUT_PKG"
