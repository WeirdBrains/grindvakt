# Changelog

All notable changes to Grindvakt are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Each release depends on the
[grindvakt-core](https://crates.io/crates/grindvakt-core) version with the same number.

## [Unreleased]

### Changed

- Releases use the shared WeirdBrains release script (`scripts/weirdnet_release.py` with
  `release.toml`) instead of `scripts/release/release.py`. The grindvakt-core lockstep checks
  are unchanged: the crate must be on crates.io first, and Cargo.lock must match its checksum.

## [1.1.4] - 2026-11-12

### Fixed

- The Windows .msi pointed the service at `C:\Grindvakt` instead of `%PROGRAMDATA%\Grindvakt`,
  so the service never found the config that `grindvakt init` writes. The installer now creates
  and uses `%PROGRAMDATA%\Grindvakt`. If you moved your config and vault to `C:\Grindvakt` to
  work around this, move `config.toml` and `vault.json` back to `%PROGRAMDATA%\Grindvakt`.
- Windows service runs on an interval are now `interval_secs` apart, not 60 s more.
- The Windows service log has no terminal color codes and records start, stop and the data dir.
- When the Windows service cannot write `service.log`, it says why in the Application event log
  (source `Grindvakt`) instead of failing silently.

### Changed

- The Windows service restarts 60 s after a crash (the MSI and `grindvakt service install`
  both set this) and has a description.
- The .msi adds the install folder to the system PATH, so `grindvakt` works in a new shell.
- Release and Pages workflows use actions/checkout 7.0.1 (without persisted credentials),
  softprops/action-gh-release 3.0.4, dart-lang/setup-dart 1.8.2 and actions/deploy-pages
  5.0.1, all pinned to commit SHAs.
- Depends on grindvakt-core 1.1.4 (pinned `=1.1.4`), which has no library changes.

## [1.1.3] - 2026-11-12

### Changed

- The macOS .pkg is Apple Silicon only. It sets `hostArchitectures="arm64"` and has an
  installation check that stops on Intel Macs with "Apple silicon required". Intel Macs are
  not supported. The packaging script refuses a binary that is not thin arm64 and checks the
  built pkg for these settings.
- README, packaging docs, the site and the release notes say Apple Silicon only.
- Depends on grindvakt-core 1.1.3 (pinned `=1.1.3`), which has no library changes.

## [1.1.2] - 2026-11-12

### Changed

- The macOS .pkg installs the license at `/usr/local/share/doc/grindvakt/LICENSE` and shows
  it in a License pane. The Windows .msi installs `LICENSE.txt` next to `grindvakt.exe` and
  shows it in the setup wizard. Both packaging scripts check the built installer for it.
- Release and Pages workflows use least-privilege `permissions:` and actions pinned to
  commit SHAs. Only the final publish job can write to the release; the macOS and Windows
  builds hand their installers to it as artifacts.
- Depends on grindvakt-core 1.1.2 (pinned `=1.1.2`), which has no library changes.

## [1.1.1] - 2026-11-11

### Changed

- Grindvakt is now open source under the BSD-3-Clause license (`LICENSE`), the same as
  grindvakt-core, and the repository is public. Package metadata declares BSD-3-Clause, and
  the Linux packages ship the license text.
- Depends on grindvakt-core 1.1.1 (pinned `=1.1.1`), which has no library changes.

## [1.1.0] - 2026-11-11

### Changed

- Depends on grindvakt-core 1.1.0 (pinned `=1.1.0`), which adds CNAME records for library
  users.
- `grindvakt sync` now refuses to write an A record at a name that is a CNAME, and says which
  target the CNAME points to, instead of creating a record beside it. Nothing is deleted.

## [1.0.0] - 2026-11-11

### Changed

- The DNS engine, vault format, config format and private-file handling now come from the
  grindvakt-core 1.0.0 crate on crates.io, pinned with `=1.0.0`.
- Scheduling stays in the app: calendar validation and the Windows service wait logic live in
  `src/schedule.rs`, and their tests run on every platform.
- Windows keeps the service-grade ACL policy (SYSTEM, Administrators, LocalService) for the data
  directory; Linux and macOS keep owner-only files.
- Minimum Rust version is 1.99.
- `scripts/release/release.py` enforces version lockstep: it refuses to release or resume unless
  grindvakt-core X.Y.Z is published on crates.io, pinned as `=X.Y.Z`, and locked with the
  crates.io checksum.

### Fixed

- DigitalOcean lookups match record names on the client, so records are found reliably.

### Compatibility

- Config and vault files are unchanged: vaults written by 0.x open in 1.0.0 and the other way
  around.
