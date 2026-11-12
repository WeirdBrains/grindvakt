#!/usr/bin/env bash
set -euo pipefail

# Prevent macOS AppleDouble (._*) and extended-attribute copies in the payload.
export COPYFILE_DISABLE=1
export COPY_EXTENDED_ATTRIBUTES_DISABLE=1

# Build an unsigned, Apple Silicon only (arm64) product archive (.pkg) containing the
# grindvakt binary, docs and the BSD-3-Clause LICENSE (installed and shown in the installer's License pane), then check
# the result. No auto-load or launchd install occurs here.

ROOT_DIR="$(cd "$(dirname "$0")"/../../.. && pwd)"
STAGE_DIR="$(mktemp -d)"
PKG_ID="com.weirdbrains.grindvakt"
VERSION="$(awk -F\\\" '/^version *=/ {print $2; exit}' "$ROOT_DIR/Cargo.toml")"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
BIN_PATH="$CARGO_TARGET_DIR/release/grindvakt"
OUT_PKG="$CARGO_TARGET_DIR/grindvakt-$VERSION-macos-arm64.pkg"
WORK_DIR="$(mktemp -d)"
cleanup() { rm -rf "$STAGE_DIR" "$WORK_DIR"; }
trap cleanup EXIT
LICENSE_SRC="$ROOT_DIR/LICENSE"
test -f "$LICENSE_SRC" || { echo "missing $LICENSE_SRC" >&2; exit 1; }

# The package is Apple Silicon only: refuse anything but a thin arm64 binary.
if command -v lipo >/dev/null 2>&1; then
  bin_archs="$(lipo -archs "$BIN_PATH")"
  [ "$bin_archs" = "arm64" ] || {
    echo "grindvakt binary must be arm64 only (Intel Macs are not supported), got: $bin_archs" >&2; exit 1; }
fi

echo "Staging payload under $STAGE_DIR"
install -d "$STAGE_DIR/usr/local/bin"
install -m 0755 "$BIN_PATH" "$STAGE_DIR/usr/local/bin/grindvakt"

install -d "$STAGE_DIR/usr/local/share/doc/grindvakt"
install -m 0644 "$LICENSE_SRC" "$STAGE_DIR/usr/local/share/doc/grindvakt/LICENSE"

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

if command -v xattr >/dev/null 2>&1; then
  xattr -cr "$STAGE_DIR" || true
fi
# Reject AppleDouble leftovers before packing
if find "$STAGE_DIR" -name '._*' -print -quit | grep -q .; then
  echo "AppleDouble files present in stage; refusing to pack" >&2
  find "$STAGE_DIR" -name '._*' -print >&2
  exit 1
fi

COMPONENT_PKG="$WORK_DIR/grindvakt-component.pkg"
echo "Building component pkg at $COMPONENT_PKG"
pkgbuild \
  --install-location / \
  --root "$STAGE_DIR" \
  --identifier "$PKG_ID" \
  --version "$VERSION" \
  "$COMPONENT_PKG"

# Product archive with a License pane: synthesize the distribution, then add the license,
# restrict it to arm64 and refuse Intel Macs with a clear message.
install -d "$WORK_DIR/resources"
install -m 0644 "$LICENSE_SRC" "$WORK_DIR/resources/LICENSE.txt"
productbuild --synthesize --package "$COMPONENT_PKG" "$WORK_DIR/distribution.xml"
cat > "$WORK_DIR/dist-extra.xml" <<EOF
    <title>Grindvakt $VERSION</title>
    <license file="LICENSE.txt" mime-type="text/plain"/>
    <installation-check script="grindvakt_arch_check()"/>
    <script><![CDATA[
function grindvakt_arch_check() {
  if (system.sysctl('hw.optional.arm64') == 1) { return true; }
  my.result.type = 'Fatal';
  my.result.title = 'Apple silicon required';
  my.result.message = 'Grindvakt $VERSION runs only on Macs with Apple silicon (M1 or later). Intel Macs are not supported.';
  return false;
}
]]></script>
EOF
awk -v extra="$WORK_DIR/dist-extra.xml" '
  { print }
  /<installer-gui-script / && !done { while ((getline l < extra) > 0) print l; done = 1 }
' "$WORK_DIR/distribution.xml" > "$WORK_DIR/distribution.tmp"
sed 's|hostArchitectures="[^"]*"|hostArchitectures="arm64"|' "$WORK_DIR/distribution.tmp" > "$WORK_DIR/distribution.xml"
check_distribution() {
  grep -q '<license file="LICENSE.txt"' "$1" || { echo "no <license> in $1" >&2; return 1; }
  grep -q 'hostArchitectures="arm64"' "$1" || { echo "hostArchitectures is not arm64 in $1" >&2; return 1; }
  ! grep -q 'x86_64' "$1" || { echo "x86_64 still listed in $1" >&2; return 1; }
  grep -q '<installation-check script="grindvakt_arch_check()"/>' "$1" || { echo "no Intel check in $1" >&2; return 1; }
}
check_distribution "$WORK_DIR/distribution.xml" || { cat "$WORK_DIR/distribution.xml" >&2; exit 1; }

echo "Building pkg at $OUT_PKG"
rm -f "$OUT_PKG"
productbuild \
  --distribution "$WORK_DIR/distribution.xml" \
  --resources "$WORK_DIR/resources" \
  --package-path "$WORK_DIR" \
  "$OUT_PKG"

# Check what was built: license pane, installed LICENSE, binary, and no AppleDouble files.
CHECK="$WORK_DIR/check"
pkgutil --expand-full "$OUT_PKG" "$CHECK"
fail() { echo "pkg check failed: $*" >&2; exit 1; }
check_distribution "$CHECK/Distribution" || fail "Distribution is missing the license or arm64-only settings"
cmp -s "$CHECK/Resources/LICENSE.txt" "$LICENSE_SRC" || fail "Resources/LICENSE.txt differs from LICENSE"
payload="$(find "$CHECK" -type d -name Payload -print -quit)"
[ -n "$payload" ] || fail "no Payload"
cmp -s "$payload/usr/local/share/doc/grindvakt/LICENSE" "$LICENSE_SRC" || fail "payload LICENSE missing or differs"
[ -x "$payload/usr/local/bin/grindvakt" ] || fail "payload binary missing"
if command -v lipo >/dev/null 2>&1; then
  [ "$(lipo -archs "$payload/usr/local/bin/grindvakt")" = "arm64" ] || fail "payload binary is not arm64 only"
fi
if pkgutil --payload-files "$COMPONENT_PKG" | grep -E '(^|/)\._' ; then
  # pkgbuild turns any remaining xattr into a ._ entry. com.apple.provenance cannot be
  # cleared and is set on files written by sandboxed or provenance-tracked processes.
  xattr -lr "$STAGE_DIR" >&2 || true
  fail "AppleDouble entries in payload (build from CI or a plain Terminal session)"
fi
if find "$CHECK" -name '._*' -print | grep . ; then fail "AppleDouble files in expanded pkg"; fi
echo "pkg checks passed: arm64 only with Intel check, license pane, /usr/local/share/doc/grindvakt/LICENSE, no ._ files"

echo "Done: $OUT_PKG"
