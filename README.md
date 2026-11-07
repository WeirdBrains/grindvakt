### grindvakt

Secure, user-mode DNS updater for DNSimple. Rust CLI with:
- Public IPv4 discovery via multiple HTTPS sources and consensus
- Idempotent A record create/update/noop on DNSimple (official crate)
- Local encrypted vault under `~/.grindvakt` with password unlock and 24‑word recovery
- systemd --user oneshot service + configurable timer (no root/system units)

No web UI. No background daemons installed automatically. No live DNS or service changes unless you run commands explicitly (supports `--dry-run`).

Quick start (development)
- Install stable Rust >= 1.87 and systemd (user manager).
- On alien-blue, prefer scratch target for builds:
  - `export CARGO_TARGET_DIR=/mnt/ext1/cargo-target` (fallback `/mnt/ext5t/cargo-target`)
  - Build: `cargo build --release`
  - Test: `cargo test` (heavy; run locally on alien-blue per notes below)
  - Cleanup after each run: `rm -rf \"$CARGO_TARGET_DIR\"`

Initialize local config and vault
- `grindvakt init` (prompts for DNSimple token, password, and emits a one-time 24-word recovery phrase — record it securely).
- Config file: `~/.grindvakt/config.toml`
- Vault: `~/.grindvakt/vault.json` (encrypted; never edit by hand)

Sync once (no writes)
- `grindvakt sync --dry-run`

Install user service without enabling
- Print units: `grindvakt service print`
- Install units: `grindvakt service install`
- Optional unattended credential enrollment using TPM2-bound systemd-creds:
  - `grindvakt service enroll-credential` (prompts for password; seals the DEK, not the password)
- To enable timer later (requires active user manager; linger remains unchanged):
  - `systemctl --user enable --now grindvakt.timer`

Important security limits
- The vault encrypts the DNSimple token using a random DEK (XChaCha20-Poly1305). The DEK is wrapped independently by an Argon2id key (from your password) and by an HKDF-SHA256 key (from your 24-word recovery). Recovery unlocks the existing vault to reset the password; losing both vault + recovery requires reissuing the token.
- Files and directory permissions are enforced (0700 dir, 0600 files) with atomic write + fsync+rename.
- No secrets on command line, env, or logs. Password prompts require a local terminal/TTY.
- Unattended mode (systemd-creds) decrypts on this machine+user once enrolled; changing only the password does not revoke the sealed DEK. Use the provided rewrap/rotation commands to revoke and reseal.
- Timers do not run when the user manager is not running (linger stays OFF).
- KDF defaults: Argon2id with 64 MiB memory, 3 iterations, p=1; bounded and validated at load.

Testing and validation (not run in cloud)
- Heavy builds/tests are not executed in this cloud environment per user request. Parent will run:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -D warnings`
  - `cargo test` (with mock DNSimple server; no live DNS)
- Use `--data-dir` to point tests to tempdirs; never touches real `~/.grindvakt`.

License
- Private draft. Not for redistribution.

