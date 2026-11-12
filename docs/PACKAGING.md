# Packaging grindvakt (Linux, macOS, Windows)

This repository ships reproducible packaging sources and scripts. Linux packages are built by the tag-triggered Release workflow. macOS Apple Silicon (arm64) `.pkg` and Windows x64 `.msi` installers are built on GitHub-hosted runners and attached to the same tag release. You can also build any of these packages locally as described below. Installers are unsigned.

Binary layout
- Binary install path: `/usr/bin/grindvakt`
- Docs: `/usr/share/doc/grindvakt/README.md`, `/usr/share/doc/grindvakt/IMPLEMENTATION_NOTES.md`
- No system-level services or config files are installed. grindvakt manages systemd --user units explicitly via its CLI.

Licensing/metadata
- Package license: `BSD-3-Clause` (Cargo.toml `license`). The `.deb` copyright file is generated from `LICENSE`; the `.rpm` and Arch packages install it to `/usr/share/licenses/grindvakt/LICENSE`. No fabricated homepage or maintainer email.

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


macOS Apple Silicon installer (.pkg)
- Apple Silicon (arm64) only; Intel Macs are not supported. The script refuses a binary that is not thin arm64, sets `hostArchitectures="arm64"` in the distribution, and adds an installation check that stops on Intel Macs with "Apple silicon required".
- The script checks the built pkg: arm64-only distribution with the Intel check, the License pane, `/usr/local/share/doc/grindvakt/LICENSE`, a thin arm64 payload binary, and no `._*` AppleDouble files.
- Script: `scripts/package/macos/make-pkg.sh`
- Requires a prior `cargo build --release` (or set `CARGO_TARGET_DIR` to a tree that already has `release/grindvakt`).
- Output: `$CARGO_TARGET_DIR/grindvakt-X.Y.Z-macos-arm64.pkg` (unsigned). The installer does not load a launch agent or enroll credentials.
- After install, enroll and install the per-user agent explicitly:
  - `grindvakt service enroll-credential`
  - `grindvakt service install`

Windows x64 installer (.msi)
- Script: `scripts/package/windows/make-msi.ps1`
- Requires the WiX Toolset 7.0.1 CLI (`wix.exe`) with `WixToolset.UI.wixext` and `WixToolset.Util.wixext` 7.0.1, and a prior `cargo build --release` producing `grindvakt.exe`.
- WiX 7 is used under the Open Source Maintenance Fee EULA v1.1 (https://docs.firegiant.com/wix/osmf/), which Michael accepted on 2026-11-12; every `wix` command passes `-acceptEula wix7` (see docs/RELEASES.md).
- Official WiX 7.0.1 CLI installer (hash must match before use; CI installs it the same way):
  - URL: `https://github.com/wixtoolset/wix/releases/download/v7.0.1/wix-cli-x64.msi`
  - SHA-256: `93673c89ab33b1fd3f07ad5e609b7cae694bd716b2914259b61a41c02303b64c`
  - `msiexec /i wix-cli-x64.msi /qn`, then `wix extension add -acceptEula wix7 -g WixToolset.UI.wixext/7.0.1 WixToolset.Util.wixext/7.0.1`; `wix.exe` is in `C:\Program Files\WiX Toolset v7.0\bin`.
- Build:
  - `powershell -File scripts/package/windows/make-msi.ps1 -ExeSource path\to\grindvakt.exe -ProductVersion X.Y.Z -OutDir dist`
  - Output: `dist/grindvakt-X.Y.Z-windows-x64.msi` (unsigned).
- The MSI installs a manual-start service running as `NT SERVICE\Grindvakt` (restart 60 s after a crash), creates a protected `%PROGRAMDATA%\Grindvakt` folder (Windows Installer's `CommonAppDataFolder`) that the service uses as its data dir, adds the install folder to the system PATH, and never auto-starts or enrolls credentials. ProgramData is `Permanent`/`NeverOverwrite`, so uninstalling keeps config, vault and credential. The data dir's protected DACL names only SYSTEM, Administrators and `NT SERVICE\Grindvakt`. Upgrades (1.1.x included, same UpgradeCode, `MajorUpgrade` after `InstallInitialize`) note whether the service was running, re-ACL everything already in `%PROGRAMDATA%\Grindvakt` for the new account (`grindvakt service repair-acl`, as SYSTEM), and start the service again if it was running. After install, in an elevated shell:
  - `grindvakt init` (writes config and vault to `%PROGRAMDATA%\Grindvakt`)
  - `grindvakt service enroll-credential` (machine-scope DPAPI)
  - `sc start Grindvakt`
- `make-msi.ps1` checks the built MSI: license, data dir under `CommonAppDataFolder`, service arguments, PATH entry, restart-on-failure actions, the service account and data dir ACL, and the order of the upgrade custom actions. The check that remembers a running service is an immediate action before `InstallInitialize`: Windows Installer refuses (error 2613) any deferred action ahead of `RemoveExistingProducts`.

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

