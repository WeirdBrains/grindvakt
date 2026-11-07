#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
. ./common.sh

usage() {
  cat <<EOF
Usage: $0 --binary /path/to/grindvakt [--version X.Y.Z] [--outdir ./dist]

Builds an RPM via cargo-generate-rpm without rebuilding sources.
Copies the provided binary into target/release/ first.
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
[[ -n "$VERSION" ]] || VERSION="0.0.0"

# Verify binary-reported version matches
BIN_VER="$("$BIN" --version 2>/dev/null | awk '{print $NF}' | sed 's/^v//')"
if [[ -n "$BIN_VER" && "$BIN_VER" != "$VERSION" ]]; then
  echo "Binary version ($BIN_VER) does not match Cargo version ($VERSION)" >&2
  exit 2
fi

# Use scratch target dir; do not leave repo-local target/
SCRATCH="$(mktemp -d)"
TMP_OUT="$(mktemp -d)"
trap 'rm -rf "$SCRATCH" "$TMP_OUT"' EXIT
mkdir -p "$SCRATCH/release" "$OUTDIR" "$TMP_OUT"
cp -f "$BIN" "$SCRATCH/release/grindvakt"
strip -s "$SCRATCH/release/grindvakt" || true

pushd "$root_dir" >/dev/null
# cargo-generate-rpm 0.21.1 has no --no-build; it respects prebuilt binaries when assets are present.
export CARGO_TARGET_DIR="$SCRATCH"
cargo generate-rpm -o "$TMP_OUT" >/dev/null
popd >/dev/null

FOUND="$(ls -1 "$TMP_OUT"/*.rpm 2>/dev/null | head -n1 || true)"
[[ -n "$FOUND" ]] || { echo "No RPM produced" >&2; exit 1; }
mv -f "$FOUND" "$OUTDIR/"
echo "$OUTDIR/$(basename "$FOUND")"

