#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
. ./common.sh

usage() {
  cat <<EOF
Usage: $0 --binary /path/to/grindvakt [--version X.Y.Z] [--outdir ./dist]

Builds an Arch .pkg.tar.zst using makepkg from a staged binary (no cargo build).
EOF
}

BIN=""
VERSION=""
OUTDIR="$root_dir/dist"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary) BIN="$2"; shift 2;;
    --version) VERSION="$2"; shift 2;;
    --outdir) OUTDIR="$2"; shift 2;;
    -h|--help) usage; exit 0;;
    *) echo "Unknown arg: $1" >&2; usage; exit 2;;
  esac
done

[[ -n "$BIN" ]] || { echo "--binary is required" >&2; exit 2; }
ensure_file "$BIN"
chmod +x "$BIN"
CRATE_VER="$(cargo_version)"
if [[ -n "${VERSION:-}" && "$VERSION" != "$CRATE_VER" ]]; then
  echo "Version mismatch: --version=$VERSION vs Cargo.toml=$CRATE_VER" >&2
  exit 2
fi
VERSION="${VERSION:-$CRATE_VER}"
BIN_VER="$("$BIN" --version 2>/dev/null | awk '{print $NF}' | sed 's/^v//')"
if [[ -n "$BIN_VER" && "$BIN_VER" != "$VERSION" ]]; then
  echo "Binary version ($BIN_VER) does not match Cargo version ($VERSION)" >&2
  exit 2
fi

WK="$(mktemp -d)"
trap 'rm -rf "$WK"' EXIT
cp -f "$BIN" "$WK/grindvakt"

cat >"$WK/PKGBUILD" <<'PKG'
pkgname=grindvakt
pkgver=0.0.0
pkgrel=1
pkgdesc="User-mode DNS updater for DNSimple (secure local vault; systemd --user)"
arch=('x86_64')
license=('custom:LicenseRef-proprietary')
depends=()  # system glibc runtime as provided by Arch
provides=('grindvakt')
conflicts=('grindvakt')
source=('local://grindvakt' 'local://README.md' 'local://IMPLEMENTATION_NOTES.md')
noextract=('grindvakt')
sha256sums=('SKIP' 'SKIP' 'SKIP')

build() {
  :
}

package() {
  install -Dm755 "$srcdir/grindvakt" "$pkgdir/usr/bin/grindvakt"
  install -Dm644 "$srcdir/README.md" "$pkgdir/usr/share/doc/grindvakt/README.md"
  install -Dm644 "$srcdir/IMPLEMENTATION_NOTES.md" "$pkgdir/usr/share/doc/grindvakt/IMPLEMENTATION_NOTES.md"
}
PKG

cp -f "$root_dir/README.md" "$WK/README.md" || true
cp -f "$root_dir/docs/IMPLEMENTATION_NOTES.md" "$WK/IMPLEMENTATION_NOTES.md" || true

# makepkg consumes "local://" via copy from working dir
pushd "$WK" >/dev/null
sed -i "s/pkgver=0.0.0/pkgver=${VERSION}/" PKGBUILD
makepkg -f --nodeps --skipinteg --skippgpcheck >/dev/null
PKGFILE="$(ls -1 *.pkg.tar.zst | head -n1)"
popd >/dev/null

mkdir -p "$OUTDIR"
cp -f "$WK/$PKGFILE" "$OUTDIR/"
echo "$OUTDIR/$PKGFILE"

