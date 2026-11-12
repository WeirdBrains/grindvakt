# Changelog

All notable changes to Grindvakt are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Each release depends on the
[grindvakt-core](https://crates.io/crates/grindvakt-core) version with the same number.

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
