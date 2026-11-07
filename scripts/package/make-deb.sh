#!/usr/bin/env bash
set -euo pipefail
echo "Deprecated. Use cargo-deb wrapper instead:" >&2
echo "  scripts/package/make-deb-cargo.sh --binary /path/to/grindvakt --version X.Y.Z --outdir dist/" >&2
exit 1

