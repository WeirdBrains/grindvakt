### Grindvakt — Implementation Notes

Key components
- `src/ip.rs`: Resolve public IPv4 via 4 independent HTTPS sources using `reqwest` with rustls. Majority consensus (>=2 agreeing) and strict IPv4 global filtering.
- `src/dnsclient.rs`: DNSimple integration via the official `dnsimple` crate (8.x). Idempotent create/update/noop with pagination and name-type exact match; safe duplicate handling and `--dry-run` planning.
- `src/vault.rs`: Local encrypted vault in `~/.grindvakt/vault.json`. Secrets (DNSimple token) are encrypted with a random 32-byte DEK using XChaCha20-Poly1305. The DEK is wrapped twice:
  - Password-derived Argon2id key (bounded params, independent salt).
  - Recovery-derived HKDF-SHA256 key from a 24-word BIP39 phrase (independent salt).
  Versioned authenticated format with clear AAD, atomic write, and strict file/dir permissions.
- `src/service.rs`: systemd --user oneshot `grindvakt.service` and configurable `grindvakt.timer`. Optional TPM2-bound unattended credential enrollment via `systemd-creds --user encrypt --with-key=host+tpm2` producing `~/.config/systemd/user/credstore.encrypted/grindvakt.dek.enc` and `LoadCredentialEncrypted=` in the unit.
- `src/config.rs`: TOML config parser and normalizer, with schedule either `interval_secs` (default 300) or `calendar`.
- `src/lock.rs`: Flock preventing overlap (service/manual).

CLI commands
- `init`: interactive config + vault creation; prints 24-word recovery and requires confirmation echo.
- `sync`: resolves current IPv4 and updates A record idempotently; supports `--dry-run` and `--no-unattended`.
- `recover`: rewrap DEK using recovery phrase to set a new password.
- `change-password`: rewrap DEK using current password.
- `service print|install|enroll-credential`: print/install units and enroll unattended DEK.

Security properties and limits
- No secrets in CLI args, env, or logs; secret prompts require a local TTY.
- Directory 0700 and files 0600, atomic temp+fsync+rename+dir fsync.
- Bounded Argon2id parameters to avoid resource-exhaustion attacks; reject unknown/tampered versions.
- Unattended mode decrypts on this machine+user after enrollment; password-only changes do not revoke access. Use rewrap/rotation to revoke and reseal DEK explicitly.
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
- TTL enforces min 60; default 300.

