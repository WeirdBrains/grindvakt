### grindvakt

Secure, user-mode DNS updater for DNSimple and DigitalOcean. Rust CLI with:
- Public IPv4 discovery via multiple HTTPS sources and consensus
- Idempotent A record create/update/noop on DNSimple (official crate)
- Local encrypted vault under `~/.grindvakt` with password unlock and 24‑word recovery
- systemd --user oneshot service + configurable timer (no root/system units)
- macOS launchd per-user agent (no autoload; install via CLI). Apple Silicon Macs only; Intel Macs are not supported
- Windows service (manual start) with a DPAPI unattended credential

The DNS engine, vault format and private-file handling live in the
[grindvakt-core](https://crates.io/crates/grindvakt-core) library
([docs](https://docs.rs/grindvakt-core), [repo](https://github.com/WeirdBrains/grindvakt-core)),
which other apps can embed. This app pins `grindvakt-core = "=X.Y.Z"` from crates.io at its own
version X.Y.Z; see `docs/RELEASES.md` for the lockstep release order.

No web UI. No background daemons installed automatically. No live DNS or service changes unless you run commands explicitly (supports `--dry-run`).

Quick start (development)
- Install stable Rust >= 1.99 and systemd (user manager).
- On alien-blue, prefer scratch target for builds:
  - `export CARGO_TARGET_DIR=/mnt/ext1/cargo-target` (fallback `/mnt/ext5t/cargo-target`)
  - Build: `cargo build --release`
  - Test: `cargo test` (heavy; run locally on alien-blue per notes below)
  - Cleanup after each run: `rm -rf \"$CARGO_TARGET_DIR\"`

Initialize local config and vault
- `grindvakt init` (prompts for provider selection, that provider's API token, password, and emits a one-time 24-word recovery phrase — record it securely).
- Config file: `~/.grindvakt/config.toml`
- Vault: `~/.grindvakt/vault.json` (encrypted; never edit by hand)

Sync once (no writes)
- `grindvakt sync --dry-run`

Install user service without enabling
- Print service/agent: `grindvakt service print`
- Install service/agent: `grindvakt service install`
- Optional unattended credential enrollment:
  - Linux: TPM2-bound systemd-creds — `grindvakt service enroll-credential` (prompts for password; seals the DEK, not the password)
  - macOS: Keychain generic password (service=grindvakt, account=grindvakt.dek) — `grindvakt service enroll-credential`
  - Linux users: If you enrolled before this fix, rerun `grindvakt service enroll-credential` once to regenerate the encrypted credential with the correct name. Old blobs may cause the service to fail at step CREDENTIALS. macOS users are not affected.
- To enable timer later on Linux (requires active user manager; linger remains unchanged):
  - `systemctl --user enable --now grindvakt.timer`

Windows service
- Manual-start Windows service that runs as its own virtual account, `NT SERVICE\\Grindvakt` (no password; nothing else runs as it).
- Program data directory: `%PROGRAMDATA%\\Grindvakt` (service log `service.log` is written here; if the service cannot write it, the reason goes to the Application event log, source `Grindvakt`).
- Unattended credential (machine-scoped DPAPI) path: `%PROGRAMDATA%\\Grindvakt\\credentials\\grindvakt.dek.dpapi`.
- Service arguments pass `--data-dir=\"%PROGRAMDATA%\\Grindvakt\"`; for unattended service runs, `--data-dir` is effectively fixed to ProgramData and not configurable.
- Install service: `grindvakt service install` (does not start it). Start later with `sc start Grindvakt`.
- Setup after the MSI (elevated shell): `grindvakt init`, then `grindvakt service enroll-credential`, then `sc start Grindvakt`. The MSI puts `grindvakt` on the system PATH.
- The service restarts 60 s after a crash. An MSI upgrade starts it again if it was running before.
- Enroll unattended credential: `grindvakt service enroll-credential` (requires elevated Administrator; seals the DEK with machine‑scope DPAPI).
- The ProgramData folder, vault and credential have a protected DACL allowing only SYSTEM, Administrators and `NT SERVICE\\Grindvakt`. User‑owned folders are rejected. Upgrades/uninstall preserve ProgramData contents. Upgrading from 1.1.x moves the service off LocalService and re-ACLs the files it left (the MSI does it; so does `grindvakt service install`).
- MSI packaging does not auto‑start services or background tasks; no activation occurs until you run explicit commands.

Important security limits
- The vault encrypts provider tokens using a random DEK (XChaCha20-Poly1305). The DEK is wrapped independently by an Argon2id key (from your password) and by an HKDF-SHA256 key (from your 24-word recovery). Recovery unlocks the existing vault to reset the password; losing both vault + recovery requires reissuing the token.
- Files and directory permissions are enforced (0700 dir, 0600 files) with atomic write + fsync+rename.
- No secrets on command line, env, or logs. Password prompts require a local terminal/TTY.
- Unattended mode: on Linux (systemd-creds) or macOS (Keychain). Changing only the password does not revoke the sealed DEK; use rewrap/rotation to revoke and reseal.
- Timers do not run when the user manager is not running (linger stays OFF). On macOS, launch agents are per-user and not auto-loaded by the installer.
- KDF defaults: Argon2id with 64 MiB memory, 3 iterations, p=1; bounded and validated at load.

Testing and validation (not run in cloud)
- Heavy builds/tests are not executed in this cloud environment per user request. Parent will run:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --all-features -D warnings`
  - `cargo test` (with mock DNSimple server; no live DNS)
- Use `--data-dir` to point tests to tempdirs; never touches real `~/.grindvakt`.

License
- BSD-3-Clause; see `LICENSE`. The grindvakt-core library is BSD-3-Clause too.



## Providers

- Show current provider: `grindvakt provider current`
- Set provider: `grindvakt provider set dnsimple|digitalocean`
- List providers and token status: `grindvakt provider list` (prompts to unlock vault)
- Set provider token: `grindvakt provider token set dnsimple|digitalocean`

Security: tokens are encrypted in the vault; migrations preserve existing DNSimple tokens. Switching providers retains both tokens. No secrets are printed or stored on argv/env.

macOS service (Apple Silicon only)
- The `.pkg` installs only on Apple Silicon (M1 or later) Macs and refuses Intel Macs.
- Print agent: `grindvakt service print` (shows the user LaunchAgent .plist)
- Install agent: `grindvakt service install` (writes to `~/Library/LaunchAgents/com.weirdbrains.grindvakt.plist`, does not load/enable)
- Load later: `launchctl load ~/Library/LaunchAgents/com.weirdbrains.grindvakt.plist`
- Logs: `~/Library/Logs/Grindvakt/{grindvakt.out.log,grindvakt.err.log}` after load.
