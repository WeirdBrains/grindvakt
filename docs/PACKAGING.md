# Packaging grindvakt (Linux, macOS, Windows)

This repository ships reproducible packaging sources and scripts. Linux packages are built by the tag-triggered Release workflow. macOS ARM64 `.pkg` and Windows x64 `.msi` installers are built on GitHub-hosted runners and attached to the same tag release. You can also build any of these packages locally as described below. Installers are unsigned.

Binary layout
- Binary install path: `/usr/bin/grindvakt`
- Docs: `/usr/share/doc/grindvakt/README.md`, `/usr/share/doc/grindvakt/IMPLEMENTATION_NOTES.md`
- No system-level services or config files are installed. grindvakt manages systemd --user units explicitly via its CLI.

Licensing/metadata
- Package license: `LicenseRef-proprietary` (Cargo.toml `license = "UNLICENSED"`). No fabricated homepage or maintainer email.

Prereqs on alien-blue
- Arch tools: `makepkg`, `fakeroot`, `bsdtar`, `zstd`
- Debian: `cargo-deb` (3.8)
- RPM: `cargo-generate-rpm` (0.21.1)
- Optional: `strip` (binutils) to reduce binary size

Build the binary (examples)
- glibc x86_64 (current system glibc ≥ 2.39):
  - `cargo build --release`
  - Binary at `target/release/grindvakt`
- musl static (optional, reliable container build):
  - `docker run --rm -v "$PWD":/src -w /src rust:1.99-alpine sh -lc 'apk add --no-cache musl-dev cmake perl make clang && rustup target add x86_64-unknown-linux-musl && RUSTFLAGS="-C target-feature=+crt-static" cargo build --release --target x86_64-unknown-linux-musl'`
  - Binary at `target/x86_64-unknown-linux-musl/release/grindvakt`

Debian (.deb) via cargo-deb (no rebuild)
- Copy prebuilt binary to `target/release/grindvakt`:
  - `install -Dm755 path/to/grindvakt target/release/grindvakt`
- Build:
  - `cargo deb --no-build --output dist/`
- Or use provided wrapper:
  - `scripts/package/make-deb-cargo.sh --binary path/to/grindvakt --version X.Y.Z --outdir dist/`

RPM (.rpm) via cargo-generate-rpm (no rebuild)
- Copy prebuilt binary to `target/release/grindvakt`:
  - `install -Dm755 path/to/grindvakt target/release/grindvakt`
- Build:
  - `cargo generate-rpm --no-build`
  - Result under `target/generate-rpm/` or `target/release/`
- Or use wrapper:
  - `scripts/package/make-rpm.sh --binary path/to/grindvakt --version X.Y.Z --outdir dist/`

Arch (.pkg.tar.zst) via makepkg (no rebuild)
- Use wrapper to stage a local binary and run `makepkg`:
  - `scripts/package/make-arch.sh --binary path/to/grindvakt --version X.Y.Z --outdir dist/`


macOS ARM64 installer (.pkg)
- Script: `scripts/package/macos/make-pkg.sh`
- Requires a prior `cargo build --release` (or set `CARGO_TARGET_DIR` to a tree that already has `release/grindvakt`).
- Output: `$CARGO_TARGET_DIR/grindvakt-X.Y.Z-macos-arm64.pkg` (unsigned). The installer does not load a launch agent or enroll credentials.
- After install, enroll and install the per-user agent explicitly:
  - `grindvakt service enroll-credential`
  - `grindvakt service install`

Windows x64 installer (.msi)
- Script: `scripts/package/windows/make-msi.ps1`
- Requires WiX v3.14.1 (`candle.exe`, `light.exe`) and a prior `cargo build --release` producing `grindvakt.exe`.
- Official WiX 3.14.1 portable zip (hash must match before use):
  - URL: `https://github.com/wixtoolset/wix3/releases/download/wix3141rtm/wix314-binaries.zip`
  - SHA-256: `6ac824e1642d6f7277d0ed7ea09411a508f6116ba6fae0aa5f2c7daa2ff43d31`
- Build:
  - `powershell -File scripts/package/windows/make-msi.ps1 -ExeSource path\to\grindvakt.exe -ProductVersion X.Y.Z -OutDir dist`
  - Output: `dist/grindvakt-X.Y.Z-windows-x64.msi` (unsigned).
- The MSI installs a manual-start LocalService, creates a protected `%PROGRAMDATA%\Grindvakt` folder, and never auto-starts or enrolls credentials. ProgramData is `Permanent`/`NeverOverwrite`. After install:
  - `grindvakt service enroll-credential` (elevated Administrator; machine-scope DPAPI)
  - `sc start Grindvakt`

Install/uninstall (examples)
- Debian:
  - `sudo dpkg -i dist/grindvakt_X.Y.Z_amd64.deb`
  - `sudo apt remove grindvakt`
- RPM:
  - `sudo rpm -Uvh --nosignature dist/grindvakt-X.Y.Z-1.x86_64.rpm`
  - `sudo rpm -e grindvakt`
- Arch:
  - `sudo pacman -U dist/grindvakt-X.Y.Z-1-x86_64.pkg.tar.zst`
  - `sudo pacman -R grindvakt`

Compatibility notes
- glibc builds are distro-targeted; ensure your runtime glibc version meets or exceeds the built binary’s requirement.
- musl static builds (optional) offer broader portability but may increase size; only use if your deployment requires it.

Service management remains explicit
- No package hook writes to user vaults, enables services, or enrolls credentials. Use:
  - `grindvakt service install`
  - `grindvakt service enroll-credential`
  - `systemctl --user enable --now grindvakt.timer` (when ready)

