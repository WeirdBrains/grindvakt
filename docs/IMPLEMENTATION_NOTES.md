### Grindvakt — Implementation Notes

Key components
- `src/ip.rs`: Resolve public IPv4 via 4 independent HTTPS sources using `reqwest` with rustls. Majority consensus (>=2 agreeing) and strict IPv4 global filtering.
- `src/dnsclient.rs`: DNSimple integration via the official `dnsimple` crate (8.x). Idempotent create/update/noop with pagination and name-type exact match; safe duplicate handling and `--dry-run` planning.
- `src/provider.rs`: Provider dispatch for DNSimple and DigitalOcean. DigitalOcean uses `reqwest` against `v2/domains/:domain/records`, with idempotent plan+create+update, A-only filtering, fixed-host pagination cap (50 pages), and fail-fast on 401/403/429.
- `src/vault.rs`: Local encrypted vault in `~/.grindvakt/vault.json`. Secrets (provider tokens) are encrypted with a random 32-byte DEK using XChaCha20-Poly1305 in a canonical JSON shape:
  `{ "dnsimple_token": "<string|null>", "digitalocean_token": "<string|null>" }`.
  Missing fields default to null to preserve legacy vaults that only have `dnsimple_token`.
  The DEK is wrapped twice:
  - Password-derived Argon2id key (bounded params, independent salt).
  - Recovery-derived HKDF-SHA256 key from a 24-word BIP39 phrase (independent salt).
  Versioned authenticated format with clear AAD, atomic write, and strict file/dir permissions.
- `src/service.rs`:
  - Linux: systemd --user oneshot `grindvakt.service` and configurable `grindvakt.timer`. Optional TPM2-bound unattended credential enrollment via `systemd-creds --user encrypt --with-key=host+tpm2` producing `~/.config/systemd/user/credstore.encrypted/grindvakt.dek.enc` and `LoadCredentialEncrypted=` in the unit.
  - macOS: per-user launchd agent `.plist` with `ProgramArguments = [<exe>, sync, --data-dir=...]`, `StartInterval` for interval schedules, and simple StartCalendarInterval mapping for `daily`. Logs to `~/Library/Logs/Grindvakt/`. CLI prints and installs the agent but does not load it.
  - Windows: SCM service under `NT AUTHORITY\\LocalService`, manual start. Program data at `%PROGRAMDATA%\\Grindvakt`; service log `service.log` lives there. Unattended credential is a machine‑scoped DPAPI blob at `%PROGRAMDATA%\\Grindvakt\\credentials\\grindvakt.dek.dpapi`. The ProgramData directory is created with a protected DACL (SYSTEM/Administrators/LocalService only); user‑owned preexisting directories are rejected.
- `src/config.rs`: TOML config parser and normalizer, with schedule either `interval_secs` (default 300) or `calendar`.
- `src/lock.rs`: Flock preventing overlap (service/manual).

CLI commands
- `init`: interactive config + vault creation; prints 24-word recovery and requires confirmation echo.
- Provider selection is part of init; token prompt matches chosen provider. DNSimple-only settings (account id, sandbox) are asked only when DNSimple is selected.
- `sync`: resolves current IPv4 and updates A record idempotently; supports `--dry-run` and `--no-unattended`.
- `recover`: rewrap DEK using recovery phrase to set a new password.
- `change-password`: rewrap DEK using current password.
- `service print|install|enroll-credential`: print/install units and enroll unattended DEK.

Security properties and limits
- No secrets in CLI args, env, or logs; secret prompts require a local TTY.
- Directory 0700 and files 0600, atomic temp+fsync+rename+dir fsync.
- Bounded Argon2id parameters to avoid resource-exhaustion attacks; reject unknown/tampered versions.
- Unattended mode decrypts on this machine+user after enrollment; password-only changes do not revoke access. Use rewrap/rotation to revoke and reseal DEK explicitly. On macOS the DEK is stored in Keychain as a generic password (service=grindvakt, account=grindvakt.dek).
- Timers require a running user manager; linger is not modified.

Testing (to be run on alien-blue; heavy runs are intentionally skipped here)
- Formatting and lints:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -D warnings`
- Unit tests:
  - `cargo test --all`
  - Tests use tempdirs via `--data-dir`; no access to real `~/.grindvakt`.
- Mock DNSimple integration:
  - `dnsclient` honors `api_base_url` so tests can run against a local mock server (e.g. `wiremock`).

Build/test workflow on alien-blue
- Set scratch target and clean after:
  - `export CARGO_TARGET_DIR=/mnt/ext1/cargo-target` (fallback `/mnt/ext5t/cargo-target`)
  - `cargo build --release && cargo test`
  - `rm -rf "$CARGO_TARGET_DIR"`

Notes
- The `dnsimple` crate APIs used: `new_client`, `identity().whoami()`, `zones().list_zone_records`, `create_zone_record`, `update_zone_record`, and `Client::set_base_url` for tests/sandboxing. Pagination is followed; filters match exact name and type A; duplicates are treated as an explicit ambiguity.
- DigitalOcean v2 API used: `GET/POST/PATCH /v2/domains/:domain/records`. Pagination is page-based and we do not follow off-host next links; a hard cap of 50 pages prevents unbounded scans.
- TTL enforces min 60; default 300.

