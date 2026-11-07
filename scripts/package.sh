#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

ART_DIR="${1:-/workspace}"
NAME="grindvakt-src-$(date +%Y%m%d-%H%M%S).tar.gz"
OUT="${ART_DIR}/${NAME}"

echo "Creating source artifact at: ${OUT}"
# Use git archive to include all tracked files (including dot-directories)
git archive --format=tar.gz --prefix="grindvakt/" -o "${OUT}" HEAD

echo "${OUT}"

