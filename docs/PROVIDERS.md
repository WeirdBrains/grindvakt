### Providers: DNSimple and DigitalOcean

- Active provider is stored in `~/.grindvakt/config.toml` under `provider = "dnsimple"|"digitalocean"`.
- Provider tokens are stored encrypted in the vault `~/.grindvakt/vault.json` in a canonical JSON form:
  `{ "dnsimple_token": "<string|null>", "digitalocean_token": "<string|null>" }`.
  - Legacy vaults with only `{"dnsimple_token":"..."}` are accepted unchanged.
  - Parsing errors bail out; no silent fallbacks.
  - Switching providers never discards the other token.

CLI
- Show current: `grindvakt provider current`
- Set active: `grindvakt provider set dnsimple|digitalocean`
- Token status: `grindvakt provider list` (prompts to unlock)
- Set token: `grindvakt provider token set dnsimple|digitalocean`

Init flow
- `grindvakt init` prompts for provider selection, then requests the corresponding token.
- DNSimple-only questions (account id and sandbox) are only asked when DNSimple is selected.
- All other prompts (zone, name, TTL, schedule) are unchanged. Enter keeps defaults; `-`/`none` clears where supported.

DigitalOcean behavior
- API base: `https://api.digitalocean.com` (override via `api_base_url` for tests only).
- Only A records are considered; exact name match with safe handling for `@`, relative labels, and FQDN.
- Pagination: page+per_page with a hard cap of 50 pages. We do not follow off-host `next` links.
- Errors: 401/403/429 fail fast; selected 5xx are retried only in DNSimple integration (DO is simple enough to bail).
- Idempotent plan: returns create/update/noop; writes are performed only when not `--dry-run`.

Security
- No tokens on argv/env/logs.
- Vault password and recovery phrase protect the DEK. Recovery rewraps the password without changing tokens.
- Unattended credential (optional) seals the DEK to the machine TPM2 via `systemd-creds`.

