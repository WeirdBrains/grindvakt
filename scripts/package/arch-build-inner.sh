#!/bin/sh
set -eu

# Expected environment from docker:
# - HOST_UID, HOST_GID: numeric ids from the host runner
# - VERSION: version string to package
# - BINARY: absolute path to built binary inside container mount (e.g. /src/target/.../grindvakt)
# - OUTDIR: absolute path to write artifacts (e.g. /src/dist)

pacman -Sy --noconfirm --needed base-devel fakeroot tar zstd python rust

# Ensure group exists or capture existing name
if ! getent group "$HOST_GID" >/dev/null 2>&1; then
  groupadd -g "$HOST_GID" buildergrp
fi

# Ensure user for the given UID exists; if present, reuse its name
U="$(getent passwd "$HOST_UID" | cut -d: -f1 || true)"
if [ -z "$U" ]; then
  useradd -m -o -u "$HOST_UID" -g "$HOST_GID" builder
  U=builder
fi

# Run makepkg flow as the host-matched user
su - "$U" -s /bin/bash -c "
  set -e
  cd /src
  echo 'Container effective user:' \$(id)
  chmod +x scripts/package/*.sh
  scripts/package/make-arch.sh \
    --binary \"$BINARY\" \
    --version \"$VERSION\" \
    --outdir \"$OUTDIR\"
"

