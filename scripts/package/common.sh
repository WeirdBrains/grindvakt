#!/usr/bin/env bash
set -euo pipefail

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

cargo_metadata_json() {
  (cd "$root_dir" && CARGO_TERM_COLOR=never cargo metadata --no-deps --offline --format-version 1 -q)
}

read_cargo_field() {
  # Prefer cargo metadata (offline) for robust parsing; portable across awk variants
  local field="$1"
  local meta
  if meta="$(cargo_metadata_json 2>/dev/null)"; then
    # Use python3 to parse JSON from stdin correctly
    local val
    val="$(python3 -c 'import sys,json; fld=sys.argv[1]; d=json.load(sys.stdin); pk=d.get("packages") or []; print(pk[0].get(fld) or "") if pk else sys.exit(1)' "$field" <<<"$meta" 2>/dev/null || true)"
    if [[ -n "${val:-}" ]]; then
      printf "%s\n" "$val"
      return 0
    fi
  fi
  # Fallback very simple parser (best-effort)
  awk -v FIELD="$field" '
    /^[ \t]*\[package\][ \t]*$/ { in_pkg=1; next }
    /^[ \t]*\[/ { if (in_pkg) in_pkg=0 }
    in_pkg && $0 ~ "^[ \t]*" FIELD "[ \t]*=" {
      line=$0
      sub(/^[ \t]*/, "", line)
      sub(FIELD "[ \t]*=[ \t]*/", "/", line)  # normalize
      sub(/^[^=]*=[ \t]*/, "", line)
      sub(/[ \t]*#.*/, "", line)
      if (line ~ /^"/) { sub(/^"/,"",line); sub(/".*$/,"",line) } else { sub(/[ \t].*$/,"",line) }
      print line; exit
    }
  ' "$root_dir/Cargo.toml"
}

cargo_version() {
  local v
  v="$(read_cargo_field "version" || true)"
  if [[ -z "${v:-}" ]]; then
    echo "Failed to read package version from Cargo.toml" >&2
    return 2
  fi
  printf "%s\n" "$v"
}

cargo_license() {
  read_cargo_field "license"
}

ensure_file() {
  local f="$1"
  [[ -f "$f" ]] || { echo "Missing file: $f" >&2; exit 1; }
}

copy_docs() {
  local dest="$1"
  mkdir -p "$dest"
  [[ -f "$root_dir/README.md" ]] && cp "$root_dir/README.md" "$dest/"
  [[ -f "$root_dir/docs/IMPLEMENTATION_NOTES.md" ]] && cp "$root_dir/docs/IMPLEMENTATION_NOTES.md" "$dest/"
  # license placeholder from Cargo.toml license string
  local lic; lic="$(cargo_license || echo UNLICENSED)"
  printf "License: %s\n\nThis package includes files from this repository.\n" "$lic" > "$dest/copyright"
}

