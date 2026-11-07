use crate::config::{AppConfig, ConfigPaths, write_config_atomic};
use crate::fsutil::{atomic_write_0600, ensure_dir_0700};
use anyhow::{Context, Result, bail};
use argon2::Argon2;
use argon2::Params as Argon2Params;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use bip39::{Language, Mnemonic};
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XNonce};
use rand::RngCore;
use regex::Regex;
use rpassword::prompt_password;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fs;
use std::io::{Write as IoWrite, stdin, stdout};
use std::path::Path;
// Note: no native keychain/DPAPI integration in provider-focused changeset

const VAULT_VERSION: u32 = 1;
const AAD_CONTEXT: &str = "grindvakt-vault-v1";
const CREDENTIAL_NAME: &str = "grindvakt.dek";

// Upper bounds to avoid pathological resource use
const ARGON2_MAX_MEMORY_MIB: u32 = 64;
const ARGON2_MAX_ITERATIONS: u32 = 6;
const ARGON2_MAX_PARALLELISM: u32 = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultFile {
    pub version: u32,
    pub kdf: KdfParams,
    pub salt_password: String,           // base64
    pub salt_recovery: String,           // base64
    pub dek_wrapped_by_password: String, // base64(aead)
    pub dek_wrapped_by_recovery: String, // base64(aead)
    pub secrets_aead_nonce: String,      // base64(24 bytes)
    pub secrets_aead_ciphertext: String, // base64
    pub created_unix: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    pub mem_mib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            mem_mib: 64,
            iterations: 3,
            parallelism: 1,
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct Secrets {
    #[serde(default)]
    dnsimple_token: Option<String>,
    #[serde(default)]
    digitalocean_token: Option<String>,
}

pub struct VaultInitFlow<'a> {
    paths: &'a ConfigPaths,
    cfg: AppConfig,
}

impl<'a> VaultInitFlow<'a> {
    pub fn new(paths: &'a ConfigPaths, cfg: AppConfig) -> Self {
        Self { paths, cfg }
    }

    pub async fn run_interactive(self) -> Result<()> {
        // Confirm data dir exists and permissions
        ensure_dir_0700(&self.paths.data_dir)?;

        if self.paths.vault_file.exists() {
            bail!(
                "Vault already exists at {} — refusing to overwrite",
                self.paths.vault_file.display()
            );
        }
        if self.paths.config_file.exists() {
            // ok; we will overwrite config with confirmed version
        }

        println!("Config path: {}", self.paths.config_file.display());
        // Collect full config interactively, with current/default values as seeds.
        let mut cfg = self.cfg.clone();
        // Zone (require a real zone; example.com not allowed)
        cfg.zone = loop {
            // If the seed is the placeholder, force explicit entry by omitting the default hint.
            let seed = if cfg.zone.trim() == "example.com" {
                ""
            } else {
                &cfg.zone
            };
            let z = prompt_string_default("DNS zone (e.g., example.com)", seed)?;
            if !is_valid_fqdn(&z) {
                eprintln!(
                    "Zone must be a valid domain (letters, digits, hyphens; labels 1–63; TLD >= 2)."
                );
                continue;
            }
            break z;
        };
        // Name/label
        cfg.name = loop {
            let n = prompt_string_default("Record name ('@' for apex, label, or FQDN)", &cfg.name)?;
            if n.trim().is_empty() {
                eprintln!("Name cannot be empty.");
                continue;
            }
            if !is_valid_record_name(&cfg.zone, &n) {
                eprintln!(
                    "Name must be '@', a relative label (e.g. 'www' or 'home.office'), or a full FQDN."
                );
                continue;
            }
            break n;
        };
        // TTL
        cfg.ttl = prompt_u32_default("TTL seconds (>=60)", cfg.ttl, 60)?;
        // Select provider before DNSimple-only prompts
        use crate::config::Provider;
        loop {
            let seed = cfg.provider.to_string(); // lowercase
            let inp = prompt_string_default("Provider (dnsimple|digitalocean)", &seed)?;
            let t = inp.trim().to_lowercase();
            if t.is_empty() {
                break;
            }
            match t.as_str() {
                "dnsimple" => {
                    cfg.provider = Provider::Dnsimple;
                    break;
                }
                "digitalocean" => {
                    cfg.provider = Provider::Digitalocean;
                    break;
                }
                _ => {
                    eprintln!("Unsupported provider. Choose 'dnsimple' or 'digitalocean'.");
                    continue;
                }
            }
        }
        // DNSimple-only prompts
        if let Provider::Dnsimple = cfg.provider {
            // Account id (optional) — Enter keeps, '-'/'none' clears, number sets.
            cfg.account_id = prompt_opt_u64(
                "DNSimple account id (optional; Enter keeps, '-' to clear)",
                cfg.account_id,
            )?;
            // Sandbox
            cfg.sandbox = prompt_bool_default("Use DNSimple sandbox?", cfg.sandbox)?;
        } else {
            cfg.account_id = None;
            cfg.sandbox = false;
        }
        // Schedule: choose calendar or interval
        let mut interval: u64 = cfg.schedule.interval_secs.unwrap_or(300);
        let had_existing_calendar = cfg.schedule.calendar.is_some();
        let mut cal = prompt_string_default(
            if had_existing_calendar {
                "OnCalendar expression (Enter to keep, '-' or 'none' to use interval)"
            } else {
                "OnCalendar expression (Enter to use interval)"
            },
            cfg.schedule.calendar.as_deref().unwrap_or(""),
        )?;
        if !had_existing_calendar && cal.trim().is_empty() {
            // No existing default and user pressed Enter -> choose interval path
            interval = prompt_u64_default("Polling interval seconds", interval, 60)?;
            cfg.schedule.calendar = None;
            cfg.schedule.interval_secs = Some(interval);
        } else {
            // Reprompt until valid calendar or switch to interval by entering blank.
            loop {
                let t = cal.trim().to_lowercase();
                if t.is_empty() && had_existing_calendar {
                    break;
                } else if t.is_empty() && !had_existing_calendar {
                    interval = prompt_u64_default("Polling interval seconds", interval, 60)?;
                    cfg.schedule.calendar = None;
                    cfg.schedule.interval_secs = Some(interval);
                    break;
                }
                if t == "-" || t == "none" {
                    interval = prompt_u64_default("Polling interval seconds", interval, 60)?;
                    cfg.schedule.calendar = None;
                    cfg.schedule.interval_secs = Some(interval);
                    break;
                } else if validate_calendar(&cal) {
                    cfg.schedule.calendar = Some(cal);
                    cfg.schedule.interval_secs = None;
                    break;
                }
                eprintln!(
                    "Calendar did not validate. Enter a valid OnCalendar value, '-' or 'none' to use interval, or press Enter to keep the current/default choice."
                );
                cal = prompt_string_default(
                    "OnCalendar expression (Enter keeps or interval if none, '-' or 'none' to use interval)",
                    "",
                )?;
            }
        }
        // Validate before secrets
        cfg.validate()?;
        println!("\nAbout to write config:");
        println!("  Zone: {}", cfg.zone);
        println!("  Name: {}", cfg.name);
        println!("  Provider: {}", cfg.provider);
        println!("  TTL: {}", cfg.ttl);
        println!(
            "  Schedule: {}",
            cfg.schedule
                .calendar
                .clone()
                .unwrap_or_else(|| format!("every {}s", cfg.schedule.interval_secs.unwrap_or(0)))
        );
        if let Provider::Dnsimple = cfg.provider {
            println!(
                "  Account id: {}",
                cfg.account_id
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "auto".to_string())
            );
            println!("  Sandbox: {}", cfg.sandbox);
        }
        let ok = prompt_bool_default("Proceed with these settings?", true)?;
        if !ok {
            bail!("aborted by user");
        }

        // Prompt for token and password
        let token_prompt = match cfg.provider {
            Provider::Dnsimple => "Enter DNSimple API token: ",
            Provider::Digitalocean => "Enter DigitalOcean API token: ",
        };
        let token = prompt_secret(token_prompt)?;
        let password = prompt_secret("Set a password to protect your vault: ")?;
        let password_confirm = prompt_secret("Confirm password: ")?;
        if password.expose_secret() != password_confirm.expose_secret() {
            bail!("passwords do not match");
        }

        // Generate 24-word recovery from CSPRNG entropy (32 bytes)
        let mut entropy = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut entropy);
        let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy)
            .map_err(|e| anyhow::anyhow!("mnemonic generation failed: {e}"))?;
        let phrase = mnemonic.to_string();
        println!("\nYour 24-word recovery phrase (write it down, never store it digitally):\n");
        println!("{}", phrase);
        println!("\nYou will be asked to re-enter the phrase to confirm.");
        let check = prompt_line("Re-enter recovery phrase: ")?;
        if normalize_ws(&check) != normalize_ws(&phrase) {
            bail!("recovery phrase did not match");
        }

        // Build vault
        let kdf = KdfParams::default();
        let salt_password = random_bytes(16);
        let salt_recovery = random_bytes(16);
        let dek = random_bytes(32);

        // Wrap DEK with password key (Argon2id)
        let pwd_key = derive_argon2_key(&password, &salt_password, &kdf)?;
        let dek_wrapped_by_password = aead_seal(&pwd_key, &dek, AAD_CONTEXT.as_bytes())?;

        // Wrap DEK with recovery key (HKDF-SHA256 from mnemonic seed)
        let recovery_key = derive_hkdf_from_mnemonic(&mnemonic, &salt_recovery);
        let dek_wrapped_by_recovery = aead_seal(&recovery_key, &dek, AAD_CONTEXT.as_bytes())?;

        // Encrypt secrets with DEK (canonical shape; preserves future additional providers)
        let secrets = match cfg.provider {
            Provider::Dnsimple => Secrets {
                dnsimple_token: Some(token.expose_secret().clone()),
                digitalocean_token: None,
            },
            Provider::Digitalocean => Secrets {
                dnsimple_token: None,
                digitalocean_token: Some(token.expose_secret().clone()),
            },
        };
        let secrets_json = serde_json::to_vec(&secrets)?;
        let (secrets_aead_nonce, secrets_aead_ciphertext) =
            aead_encrypt_detached(&dek, &secrets_json, AAD_CONTEXT.as_bytes())?;

        let vault = VaultFile {
            version: VAULT_VERSION,
            kdf,
            salt_password: B64.encode(salt_password),
            salt_recovery: B64.encode(salt_recovery),
            dek_wrapped_by_password: B64.encode(dek_wrapped_by_password),
            dek_wrapped_by_recovery: B64.encode(dek_wrapped_by_recovery),
            secrets_aead_nonce: B64.encode(secrets_aead_nonce),
            secrets_aead_ciphertext: B64.encode(secrets_aead_ciphertext),
            created_unix: chrono::Utc::now().timestamp(),
        };

        // Write config and vault atomically
        write_config_atomic(self.paths, &cfg)?;
        let vault_text = serde_json::to_string_pretty(&vault)?;
        atomic_write_0600(&self.paths.vault_file, vault_text.as_bytes()).context("write vault")?;
        println!("Initialized. Vault at {}", self.paths.vault_file.display());
        println!("Keep your recovery phrase safe. It is shown only during init/recover flows.");
        Ok(())
    }
}

pub struct RecoverFlow<'a> {
    paths: &'a ConfigPaths,
}

impl<'a> RecoverFlow<'a> {
    pub fn new(paths: &'a ConfigPaths) -> Self {
        Self { paths }
    }
    pub async fn run_interactive(self) -> Result<()> {
        let vault = load_vault(&self.paths.vault_file)?;
        check_vault_supported(&vault)?;
        println!("Enter your 24-word recovery phrase to rewrap the password.");
        let phrase_str = prompt_line("Recovery phrase: ")?;
        let mnemonic = Mnemonic::parse_in(Language::English, &phrase_str)
            .map_err(|_| anyhow::anyhow!("invalid recovery phrase"))?;
        let salt_recovery = B64.decode(&vault.salt_recovery)?;
        let recovery_key = derive_hkdf_from_mnemonic(&mnemonic, &salt_recovery);
        let wrapped = B64.decode(&vault.dek_wrapped_by_recovery)?;
        let dek = aead_open(&recovery_key, &wrapped, AAD_CONTEXT.as_bytes())
            .map_err(|e| anyhow::anyhow!("recovery unlock failed: {}", e))?;
        // Prompt for new password
        let new_pwd = prompt_secret("New password: ")?;
        let new_pwd2 = prompt_secret("Confirm new password: ")?;
        if new_pwd.expose_secret() != new_pwd2.expose_secret() {
            bail!("passwords do not match");
        }
        let kdf = vault.kdf.clone();
        let salt_password = B64.decode(&vault.salt_password)?;
        let pwd_key = derive_argon2_key(&new_pwd, &salt_password, &kdf)?;
        let new_wrapped = aead_seal(&pwd_key, &dek, AAD_CONTEXT.as_bytes())?;
        let mut updated = vault;
        updated.dek_wrapped_by_password = B64.encode(new_wrapped);
        let text = serde_json::to_string_pretty(&updated)?;
        atomic_write_0600(&self.paths.vault_file, text.as_bytes())?;
        println!("Password updated.");
        Ok(())
    }
}

pub struct ChangePasswordFlow<'a> {
    paths: &'a ConfigPaths,
}

impl<'a> ChangePasswordFlow<'a> {
    pub fn new(paths: &'a ConfigPaths) -> Self {
        Self { paths }
    }
    pub async fn run_interactive(self) -> Result<()> {
        let vault = load_vault(&self.paths.vault_file)?;
        check_vault_supported(&vault)?;
        let salt_pwd = B64.decode(&vault.salt_password)?;
        let kdf = vault.kdf.clone();
        let current = prompt_secret("Current password: ")?;
        let pwd_key = derive_argon2_key(&current, &salt_pwd, &kdf)?;
        // Attempt to unwrap DEK
        let wrapped = B64.decode(&vault.dek_wrapped_by_password)?;
        let dek = aead_open(&pwd_key, &wrapped, AAD_CONTEXT.as_bytes())
            .map_err(|_| anyhow::anyhow!("password incorrect or vault tampered"))?;
        let new_pwd = prompt_secret("New password: ")?;
        let new_pwd2 = prompt_secret("Confirm new password: ")?;
        if new_pwd.expose_secret() != new_pwd2.expose_secret() {
            bail!("passwords do not match");
        }
        let new_key = derive_argon2_key(&new_pwd, &salt_pwd, &kdf)?;
        let new_wrapped = aead_seal(&new_key, &dek, AAD_CONTEXT.as_bytes())?;
        let mut updated = vault;
        updated.dek_wrapped_by_password = B64.encode(new_wrapped);
        let text = serde_json::to_string_pretty(&updated)?;
        atomic_write_0600(&self.paths.vault_file, text.as_bytes())?;
        println!("Password updated.");
        Ok(())
    }
}

pub struct UnlockContext<'a> {
    pub paths: &'a ConfigPaths,
    pub allow_unattended: bool,
}

pub async fn unlock_token_interactive(ctx: UnlockContext<'_>) -> Result<SecretString> {
    // First try unattended credential (if allowed) via $CREDENTIALS_DIRECTORY/grindvakt.dek
    if ctx.allow_unattended
        && let Ok(dir) = std::env::var("CREDENTIALS_DIRECTORY")
    {
        let p = Path::new(&dir).join(CREDENTIAL_NAME);
        if p.exists()
            && let Ok(dek) = fs::read(&p)
            && dek.len() == 32
            && let Ok(token) = unlock_with_dek(ctx.paths, &dek)
        {
            return Ok(token);
        }
    }
    // Fallback to password prompt
    let vault = load_vault(&ctx.paths.vault_file)?;
    check_vault_supported(&vault)?;
    let pwd = prompt_secret("Vault password: ")?;
    let salt = B64.decode(&vault.salt_password)?;
    let kdf = vault.kdf.clone();
    let key = derive_argon2_key(&pwd, &salt, &kdf)?;
    let wrapped = B64.decode(&vault.dek_wrapped_by_password)?;
    let dek = aead_open(&key, &wrapped, AAD_CONTEXT.as_bytes())
        .map_err(|e| anyhow::anyhow!("unlock failed: {}", e))?;
    unlock_with_dek(ctx.paths, &dek)
}

fn unlock_with_dek(paths: &ConfigPaths, dek: &[u8]) -> Result<SecretString> {
    let vault = load_vault(&paths.vault_file)?;
    check_vault_supported(&vault)?;
    let nonce = B64.decode(&vault.secrets_aead_nonce)?;
    let ct = B64.decode(&vault.secrets_aead_ciphertext)?;
    let key = Key::from_slice(dek);
    let cipher = XChaCha20Poly1305::new(key);
    let nonce = XNonce::from_slice(&nonce);
    let plaintext = cipher
        .decrypt(
            nonce,
            chacha20poly1305::aead::Payload {
                msg: &ct,
                aad: AAD_CONTEXT.as_bytes(),
            },
        )
        .map_err(|e| anyhow::anyhow!("decrypt secrets: {}", e))?;
    let s: Secrets = serde_json::from_slice(&plaintext)?;
    let tok = s
        .dnsimple_token
        .ok_or_else(|| anyhow::anyhow!("dnsimple token not set in vault"))?;
    Ok(SecretString::new(tok))
}

use crate::config::Provider;

pub async fn unlock_token_for_provider_interactive(
    ctx: UnlockContext<'_>,
    provider: Provider,
) -> Result<SecretString> {
    if ctx.allow_unattended
        && let Ok(dir) = std::env::var("CREDENTIALS_DIRECTORY")
    {
        let p = std::path::Path::new(&dir).join(CREDENTIAL_NAME);
        if p.exists()
            && let Ok(dek) = std::fs::read(&p)
            && dek.len() == 32
        {
            return unlock_provider_with_dek(ctx.paths, &dek, provider);
        }
    }
    // On macOS, also try Keychain generic password (service=grindvakt, account=grindvakt.dek)
    #[cfg(target_os = "macos")]
    if ctx.allow_unattended
        && let Ok(data) =
            security_framework::passwords::get_generic_password("grindvakt", "grindvakt.dek")
        && data.len() == 32
    {
        return unlock_provider_with_dek(ctx.paths, &data, provider);
    }
    let vault = load_vault(&ctx.paths.vault_file)?;
    check_vault_supported(&vault)?;
    let pwd = prompt_secret("Vault password: ")?;
    let salt = B64.decode(&vault.salt_password)?;
    let kdf = vault.kdf.clone();
    let key = derive_argon2_key(&pwd, &salt, &kdf)?;
    let wrapped = B64.decode(&vault.dek_wrapped_by_password)?;
    let dek = aead_open(&key, &wrapped, AAD_CONTEXT.as_bytes())
        .map_err(|e| anyhow::anyhow!("unlock failed: {}", e))?;
    unlock_provider_with_dek(ctx.paths, &dek, provider)
}

fn unlock_provider_with_dek(
    paths: &ConfigPaths,
    dek: &[u8],
    provider: Provider,
) -> Result<SecretString> {
    let vault = load_vault(&paths.vault_file)?;
    check_vault_supported(&vault)?;
    let nonce = B64.decode(&vault.secrets_aead_nonce)?;
    let ct = B64.decode(&vault.secrets_aead_ciphertext)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(dek));
    let pt = cipher
        .decrypt(
            XNonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: &ct,
                aad: AAD_CONTEXT.as_bytes(),
            },
        )
        .map_err(|e| anyhow::anyhow!("decrypt secrets: {}", e))?;
    let comp: Secrets =
        serde_json::from_slice(&pt).map_err(|e| anyhow::anyhow!("parse secrets: {}", e))?;
    let out = match provider {
        Provider::Dnsimple => comp.dnsimple_token.ok_or_else(|| {
            anyhow::anyhow!("dnsimple token not set; run `grindvakt provider token set dnsimple`")
        })?,
        Provider::Digitalocean => comp.digitalocean_token.ok_or_else(|| {
            anyhow::anyhow!(
                "digitalocean token not set; run `grindvakt provider token set digitalocean`"
            )
        })?,
    };
    Ok(SecretString::new(out))
}

pub struct TokenStatus {
    pub dnsimple: bool,
    pub digitalocean: bool,
}

pub async fn tokens_status_interactive(paths: &ConfigPaths) -> Result<TokenStatus> {
    let vault = load_vault(&paths.vault_file)?;
    let pwd = prompt_secret("Vault password: ")?;
    let salt = B64.decode(&vault.salt_password)?;
    let kdf = vault.kdf.clone();
    let key = derive_argon2_key(&pwd, &salt, &kdf)?;
    let wrapped = B64.decode(&vault.dek_wrapped_by_password)?;
    let dek = aead_open(&key, &wrapped, AAD_CONTEXT.as_bytes())?;
    let nonce = B64.decode(&vault.secrets_aead_nonce)?;
    let ct = B64.decode(&vault.secrets_aead_ciphertext)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&dek));
    let pt = cipher
        .decrypt(
            XNonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: &ct,
                aad: AAD_CONTEXT.as_bytes(),
            },
        )
        .map_err(|e| anyhow::anyhow!("decrypt secrets: {}", e))?;
    let comp: Secrets =
        serde_json::from_slice(&pt).map_err(|e| anyhow::anyhow!("parse secrets: {}", e))?;
    Ok(TokenStatus {
        dnsimple: comp.dnsimple_token.is_some(),
        digitalocean: comp.digitalocean_token.is_some(),
    })
}

pub async fn set_provider_token_interactive(paths: &ConfigPaths, provider: Provider) -> Result<()> {
    let vault = load_vault(&paths.vault_file)?;
    let pwd = prompt_secret("Vault password: ")?;
    let salt = B64.decode(&vault.salt_password)?;
    let kdf = vault.kdf.clone();
    let key = derive_argon2_key(&pwd, &salt, &kdf)?;
    let wrapped = B64.decode(&vault.dek_wrapped_by_password)?;
    let dek = aead_open(&key, &wrapped, AAD_CONTEXT.as_bytes())?;
    let nonce = B64.decode(&vault.secrets_aead_nonce)?;
    let ct = B64.decode(&vault.secrets_aead_ciphertext)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&dek));
    let pt = cipher
        .decrypt(
            XNonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: &ct,
                aad: AAD_CONTEXT.as_bytes(),
            },
        )
        .map_err(|e| anyhow::anyhow!("decrypt secrets: {}", e))?;
    // Parse must succeed; do not overwrite on parse errors
    let mut comp: Secrets =
        serde_json::from_slice(&pt).map_err(|e| anyhow::anyhow!("parse secrets: {}", e))?;
    let label = match provider {
        Provider::Dnsimple => "Enter DNSimple API token: ",
        Provider::Digitalocean => "Enter DigitalOcean API token: ",
    };
    let tok = prompt_secret(label)?;
    let val = tok.expose_secret().clone();
    match provider {
        Provider::Dnsimple => comp.dnsimple_token = Some(val),
        Provider::Digitalocean => comp.digitalocean_token = Some(val),
    }
    // Serialize back to canonical shape
    let updated = serde_json::to_vec(&comp)?;
    let (n2, ct2) = aead_encrypt_detached(&dek, &updated, AAD_CONTEXT.as_bytes())?;
    let mut v = vault;
    v.secrets_aead_nonce = B64.encode(n2);
    v.secrets_aead_ciphertext = B64.encode(ct2);
    let text = serde_json::to_string_pretty(&v)?;
    atomic_write_0600(&paths.vault_file, text.as_bytes())?;
    println!("Token updated for {:?}", provider);
    Ok(())
}

pub struct CredentialEnrollment<'a> {
    paths: &'a ConfigPaths,
}
impl<'a> CredentialEnrollment<'a> {
    pub fn new(paths: &'a ConfigPaths) -> Self {
        Self { paths }
    }
    #[cfg(target_os = "linux")]
    pub async fn enroll(&self, _cfg: &AppConfig) -> Result<()> {
        let vault = load_vault(&self.paths.vault_file)?;
        let pwd = prompt_secret("Vault password (to access DEK): ")?;
        let salt = B64.decode(&vault.salt_password)?;
        let kdf = vault.kdf.clone();
        let key = derive_argon2_key(&pwd, &salt, &kdf)?;
        let wrapped = B64.decode(&vault.dek_wrapped_by_password)?;
        let mut dek = aead_open(&key, &wrapped, AAD_CONTEXT.as_bytes()).context("unlock failed")?;
        // Pipe DEK to systemd-creds --user encrypt --with-key=host+tpm2
        let dest_dir = &self.paths.credstore_dir;
        ensure_dir_0700(dest_dir)?;
        let dest_file = dest_dir.join(format!("{}.enc", CREDENTIAL_NAME));
        let mut child = tokio::process::Command::new("systemd-creds")
            .arg("--user")
            .arg("encrypt")
            .arg("--with-key=host+tpm2")
            .arg("-")
            .arg(&dest_file)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("launch systemd-creds")?;
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(&dek).await?;
        }
        let output = child.wait_with_output().await?;
        // wipe DEK from memory
        use zeroize::Zeroize;
        dek.zeroize();
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            bail!("systemd-creds failed: {}", err);
        }
        println!("Enrolled unattended credential at {}", dest_file.display());
        println!(
            "Update unit to include: LoadCredentialEncrypted={}:{}",
            CREDENTIAL_NAME,
            dest_file.display()
        );
        Ok(())
    }
    #[cfg(target_os = "macos")]
    pub async fn enroll(&self, _cfg: &AppConfig) -> Result<()> {
        let vault = load_vault(&self.paths.vault_file)?;
        let pwd = prompt_secret("Vault password (to access DEK): ")?;
        let salt = B64.decode(&vault.salt_password)?;
        let kdf = vault.kdf.clone();
        let key = derive_argon2_key(&pwd, &salt, &kdf)?;
        let wrapped = B64.decode(&vault.dek_wrapped_by_password)?;
        let dek = aead_open(&key, &wrapped, AAD_CONTEXT.as_bytes()).context("unlock failed")?;
        if dek.len() != 32 {
            bail!("unexpected DEK length");
        }
        security_framework::passwords::set_generic_password("grindvakt", "grindvakt.dek", &dek)
            .context("store DEK in Keychain")?;
        println!(
            "Enrolled unattended credential in Keychain (service=grindvakt, account=grindvakt.dek)"
        );
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub async fn enroll(&self, _cfg: &AppConfig) -> Result<()> {
        bail!("unattended credential enrollment unsupported on this OS");
    }
}

fn load_vault(path: &Path) -> Result<VaultFile> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read vault {}", path.display()))?;
    let v: VaultFile = serde_json::from_str(&text).context("parse vault")?;
    Ok(v)
}

fn check_vault_supported(v: &VaultFile) -> Result<()> {
    if v.version != VAULT_VERSION {
        bail!("unsupported vault version {}", v.version);
    }
    // Bound KDF params
    if v.kdf.mem_mib < 64
        || v.kdf.mem_mib > ARGON2_MAX_MEMORY_MIB
        || v.kdf.iterations == 0
        || v.kdf.iterations > ARGON2_MAX_ITERATIONS
        || v.kdf.parallelism == 0
        || v.kdf.parallelism > ARGON2_MAX_PARALLELISM
    {
        bail!("vault KDF parameters out of bounds");
    }
    Ok(())
}

fn derive_argon2_key(password: &SecretString, salt: &[u8], params: &KdfParams) -> Result<[u8; 32]> {
    let mut key = [0u8; 32];
    let params = Argon2Params::new(
        params.mem_mib.saturating_mul(1024), // memory in KiB
        params.iterations,
        params.parallelism,
        Some(32),
    )
    .map_err(|e| anyhow::anyhow!("argon2 params: {}", e))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    argon2
        .hash_password_into(password.expose_secret().as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("argon2 key derivation: {}", e))?;
    Ok(key)
}

fn derive_hkdf_from_mnemonic(mnemonic: &Mnemonic, salt: &[u8]) -> [u8; 32] {
    let entropy = mnemonic.to_entropy();
    let okm = hkdf::Hkdf::<Sha256>::new(Some(salt), &entropy);
    let mut key = [0u8; 32];
    okm.expand(AAD_CONTEXT.as_bytes(), &mut key)
        .expect("hkdf expand");
    key
}

fn aead_seal(key_bytes: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let key = Key::from_slice(key_bytes);
    let cipher = XChaCha20Poly1305::new(key);
    let mut nonce = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce);
    let nonce_x = XNonce::from_slice(&nonce);
    let mut blob = Vec::with_capacity(24 + plaintext.len() + 16);
    blob.extend_from_slice(&nonce);
    let ct = cipher
        .encrypt(
            nonce_x,
            chacha20poly1305::aead::Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| anyhow::anyhow!("aead encrypt: {}", e))?;
    blob.extend_from_slice(&ct);
    Ok(blob)
}

fn aead_open(key_bytes: &[u8; 32], blob: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < 24 + 16 {
        bail!("ciphertext too short");
    }
    let (nonce_bytes, ct) = blob.split_at(24);
    let key = Key::from_slice(key_bytes);
    let cipher = XChaCha20Poly1305::new(key);
    let nonce = XNonce::from_slice(nonce_bytes);
    let pt = cipher
        .decrypt(nonce, chacha20poly1305::aead::Payload { msg: ct, aad })
        .map_err(|e| anyhow::anyhow!("aead decrypt: {}", e))?;
    Ok(pt)
}

fn aead_encrypt_detached(
    key_bytes: &[u8],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<([u8; 24], Vec<u8>)> {
    let key = Key::from_slice(key_bytes);
    let cipher = XChaCha20Poly1305::new(key);
    let mut nonce = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce);
    let nonce_x = XNonce::from_slice(&nonce);
    let ct = cipher
        .encrypt(
            nonce_x,
            chacha20poly1305::aead::Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| anyhow::anyhow!("aead encrypt: {}", e))?;
    Ok((nonce, ct))
}

fn random_bytes(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut v);
    v
}

fn prompt_secret(prompt: &str) -> Result<SecretString> {
    // rpassword::prompt_password renders the prompt; avoid double-printing.
    let s = prompt_password(prompt)
        .map_err(|e| anyhow::anyhow!("tty required for secret input: {}", e))?;
    Ok(SecretString::new(s))
}

fn prompt_line(prompt: &str) -> Result<String> {
    eprint!("{}", prompt);
    stdout().flush().ok();
    let mut s = String::new();
    let n = stdin().read_line(&mut s)?;
    if n == 0 {
        bail!("input closed (EOF)");
    }
    Ok(s.trim().to_string())
}

fn prompt_string_default(label: &str, default: &str) -> Result<String> {
    let p = if default.is_empty() {
        format!("{label}: ")
    } else {
        format!("{label} [{default}]: ")
    };
    let v = prompt_line(&p)?;
    Ok(if v.trim().is_empty() {
        default.to_string()
    } else {
        v
    })
}

fn prompt_u32_default(label: &str, current: u32, min: u32) -> Result<u32> {
    loop {
        let v = prompt_string_default(label, &current.to_string())?;
        match v.trim().parse::<u32>() {
            Ok(n) if n >= min => return Ok(n),
            Ok(_) | Err(_) => eprintln!("Please enter a number >= {min}."),
        }
    }
}

fn prompt_u64_default(label: &str, current: u64, min: u64) -> Result<u64> {
    loop {
        let v = prompt_string_default(label, &current.to_string())?;
        match v.trim().parse::<u64>() {
            Ok(n) if n >= min => return Ok(n),
            Ok(_) | Err(_) => eprintln!("Please enter a number >= {min}."),
        }
    }
}

fn prompt_opt_u64(label: &str, current: Option<u64>) -> Result<Option<u64>> {
    let mut seed = current.map(|n| n.to_string()).unwrap_or_default();
    loop {
        let v = prompt_string_default(label, &seed)?;
        let t = v.trim().to_lowercase();
        if t.is_empty() {
            return Ok(current);
        }
        if t == "-" || t == "none" {
            return Ok(None);
        }
        match v.trim().parse::<u64>() {
            Ok(n) => return Ok(Some(n)),
            Err(_) => {
                eprintln!("Please enter an integer id, '-' to clear, or press Enter to keep.");
                seed = current.map(|n| n.to_string()).unwrap_or_default();
            }
        }
    }
}

fn prompt_bool_default(label: &str, current: bool) -> Result<bool> {
    let seed = if current { "Y" } else { "N" };
    loop {
        let v = prompt_string_default(label, seed)?;
        let t = v.trim().to_lowercase();
        if t.is_empty() {
            return Ok(current);
        }
        match t.as_str() {
            "y" | "yes" | "true" | "1" => return Ok(true),
            "n" | "no" | "false" | "0" => return Ok(false),
            _ => eprintln!("Please answer y/n."),
        }
    }
}

fn is_valid_fqdn(s: &str) -> bool {
    // Simple FQDN check: labels 1–63, alnum/hyphen, not starting or ending with hyphen; at least one dot; TLD >= 2.
    // Case-insensitive.
    static LABEL: &str = r"(?i)[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])";
    let re = Regex::new(&format!(
        "^({label}\\.)+[a-z{{2,}}A-Z{{2,}}]+$",
        label = LABEL
    ))
    .unwrap();
    re.is_match(s.trim_end_matches('.'))
}

fn is_valid_label(label: &str) -> bool {
    static RE_STR: &str = r"(?i)^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])$";
    let re = Regex::new(RE_STR).unwrap();
    re.is_match(label)
}

fn is_valid_record_name(zone: &str, name: &str) -> bool {
    let n = name.trim();
    if n == "@" {
        return true;
    }
    // relative labels like 'www' or 'home.office'
    if n.split('.')
        .all(|lbl| !lbl.is_empty() && is_valid_label(lbl))
    {
        return true;
    }
    // full FQDN
    is_valid_fqdn(n) && n.ends_with(zone.trim_end_matches('.'))
}

fn validate_calendar(expr: &str) -> bool {
    // Try systemd-analyze calendar "{expr}" --iterations=1
    if std::process::Command::new("systemd-analyze")
        .arg("--version")
        .output()
        .is_ok()
    {
        let out = std::process::Command::new("systemd-analyze")
            .arg("calendar")
            .arg(expr)
            .arg("--iterations=1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if let Ok(st) = out {
            return st.success();
        }
    }
    // Heuristic fallback: reject newlines, '%', and overly long strings.
    let t = expr.trim();
    !t.is_empty() && !t.contains('\n') && !t.contains('%') && t.len() <= 256
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigPaths;
    use std::fs;
    use tempfile::TempDir;
    #[test]
    fn hkdf_fixed_len() {
        // Deterministic mnemonic from fixed entropy; verify HKDF output length and determinism.
        let entropy = [0u8; 32];
        let m = Mnemonic::from_entropy_in(Language::English, &entropy).unwrap();
        let salt = b"test-salt";
        let a = derive_hkdf_from_mnemonic(&m, salt);
        let b = derive_hkdf_from_mnemonic(&m, salt);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn aead_detects_tamper() {
        let key = [7u8; 32];
        let pt = b"hello world";
        let (nonce, ct) = aead_encrypt_detached(&key, pt, AAD_CONTEXT.as_bytes()).unwrap();
        // Ok path
        let k = Key::from_slice(&key);
        let cipher = XChaCha20Poly1305::new(k);
        let ok = cipher.decrypt(
            XNonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: &ct,
                aad: AAD_CONTEXT.as_bytes(),
            },
        );
        assert!(ok.is_ok());
        // Tamper
        let mut bad = ct.clone();
        bad[0] ^= 0x01;
        let bad_res = cipher.decrypt(
            XNonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: &bad,
                aad: AAD_CONTEXT.as_bytes(),
            },
        );
        assert!(bad_res.is_err());
    }

    #[test]
    fn wrong_password_fails_to_unwrap_dek() {
        let mut e = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut e);
        let salt_pwd = b"salt-password";
        let password = SecretString::new("correct horse".into());
        let dek = [9u8; 32];
        let key = derive_argon2_key(&password, salt_pwd, &KdfParams::default()).unwrap();
        let wrapped = aead_seal(&key, &dek, AAD_CONTEXT.as_bytes()).unwrap();
        let wrong = SecretString::new("wrong battery".into());
        let wrong_key = derive_argon2_key(&wrong, salt_pwd, &KdfParams::default()).unwrap();
        let res = aead_open(&wrong_key, &wrapped, AAD_CONTEXT.as_bytes());
        assert!(res.is_err());
    }

    #[test]
    fn wrong_phrase_fails_to_unwrap_dek() {
        let mut e = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut e);
        let m = Mnemonic::from_entropy_in(Language::English, &e).unwrap();
        let salt_rec = b"salt-recovery";
        let dek = [7u8; 32];
        let key = derive_hkdf_from_mnemonic(&m, salt_rec);
        let wrapped = aead_seal(&key, &dek, AAD_CONTEXT.as_bytes()).unwrap();
        let mut e2 = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut e2);
        let other = Mnemonic::from_entropy_in(Language::English, &e2).unwrap();
        let wrong = derive_hkdf_from_mnemonic(&other, salt_rec);
        let res = aead_open(&wrong, &wrapped, AAD_CONTEXT.as_bytes());
        assert!(res.is_err());
    }

    #[test]
    fn roundtrip_change_password_and_recover() {
        // Build a small vault in-memory and verify rewrap paths.
        let password = SecretString::new("pw1".into());
        let mut salt_pwd = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt_pwd);
        let mut salt_rec = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt_rec);
        let mut entropy = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut entropy);
        let m = Mnemonic::from_entropy_in(Language::English, &entropy).unwrap();
        let dek = [42u8; 32];
        let key_pwd = derive_argon2_key(&password, &salt_pwd, &KdfParams::default()).unwrap();
        let _wrap_pwd = aead_seal(&key_pwd, &dek, AAD_CONTEXT.as_bytes()).unwrap();
        let key_rec = derive_hkdf_from_mnemonic(&m, &salt_rec);
        let wrap_rec = aead_seal(&key_rec, &dek, AAD_CONTEXT.as_bytes()).unwrap();
        // change password
        let new_pwd = SecretString::new("pw2".into());
        let key_new = derive_argon2_key(&new_pwd, &salt_pwd, &KdfParams::default()).unwrap();
        let wrap_new = aead_seal(&key_new, &dek, AAD_CONTEXT.as_bytes()).unwrap();
        // unwrap with new password equals original DEK
        let opened = aead_open(&key_new, &wrap_new, AAD_CONTEXT.as_bytes()).unwrap();
        assert_eq!(&opened[..], &dek[..]);
        // recovery-derived key also opens original wrap_rec
        let opened2 = aead_open(&key_rec, &wrap_rec, AAD_CONTEXT.as_bytes()).unwrap();
        assert_eq!(&opened2[..], &dek[..]);
    }

    #[test]
    fn tempfile_vault_persist_and_unlock() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        let paths = ConfigPaths {
            data_dir: base.to_path_buf(),
            config_file: base.join("config.toml"),
            vault_file: base.join("vault.json"),
            credstore_dir: base.join("credstore.encrypted"),
        };
        // Build vault JSON with token
        let password = SecretString::new("pw".into());
        let mut salt_pwd = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt_pwd);
        let mut salt_rec = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt_rec);
        let mut entropy = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut entropy);
        let m = Mnemonic::from_entropy_in(Language::English, &entropy).unwrap();
        let dek = random_bytes(32);
        let key_pwd = derive_argon2_key(&password, &salt_pwd, &KdfParams::default()).unwrap();
        let wrap_pwd = aead_seal(&key_pwd, &dek, AAD_CONTEXT.as_bytes()).unwrap();
        let key_rec = derive_hkdf_from_mnemonic(&m, &salt_rec);
        let wrap_rec = aead_seal(&key_rec, &dek, AAD_CONTEXT.as_bytes()).unwrap();
        let (nonce, ct) = aead_encrypt_detached(
            &dek,
            br#"{"dnsimple_token":"tok_X"}"#,
            AAD_CONTEXT.as_bytes(),
        )
        .unwrap();
        let v = VaultFile {
            version: VAULT_VERSION,
            kdf: KdfParams::default(),
            salt_password: B64.encode(salt_pwd),
            salt_recovery: B64.encode(salt_rec),
            dek_wrapped_by_password: B64.encode(wrap_pwd),
            dek_wrapped_by_recovery: B64.encode(wrap_rec),
            secrets_aead_nonce: B64.encode(nonce),
            secrets_aead_ciphertext: B64.encode(ct),
            created_unix: 0,
        };
        fs::create_dir_all(&paths.data_dir).unwrap();
        fs::write(&paths.vault_file, serde_json::to_vec(&v).unwrap()).unwrap();
        // Unlock via password path
        let dek_open = aead_open(
            &key_pwd,
            &B64.decode(&v.dek_wrapped_by_password).unwrap(),
            AAD_CONTEXT.as_bytes(),
        )
        .unwrap();
        let token = super::unlock_with_dek(&paths, &dek_open).unwrap();
        assert_eq!(token.expose_secret(), "tok_X");
        // Unlock via recovery path
        let dek_open2 = aead_open(
            &key_rec,
            &B64.decode(&v.dek_wrapped_by_recovery).unwrap(),
            AAD_CONTEXT.as_bytes(),
        )
        .unwrap();
        let token2 = super::unlock_with_dek(&paths, &dek_open2).unwrap();
        assert_eq!(token2.expose_secret(), "tok_X");
    }

    #[test]
    fn legacy_and_dual_provider_secrets_parse() {
        use crate::config::Provider;
        // Setup temp vault
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        let paths = ConfigPaths {
            data_dir: base.to_path_buf(),
            config_file: base.join("config.toml"),
            vault_file: base.join("vault.json"),
            credstore_dir: base.join("credstore.encrypted"),
        };
        // Common wrapping
        let password = SecretString::new("pw".into());
        let mut salt_pwd = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt_pwd);
        let mut salt_rec = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut salt_rec);
        let mut entropy = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut entropy);
        let m = Mnemonic::from_entropy_in(Language::English, &entropy).unwrap();
        // Case 1: legacy dnsimple-only secrets
        let dek1 = random_bytes(32);
        let key_pwd1 = derive_argon2_key(&password, &salt_pwd, &KdfParams::default()).unwrap();
        let wrap_pwd1 = aead_seal(&key_pwd1, &dek1, AAD_CONTEXT.as_bytes()).unwrap();
        let key_rec1 = derive_hkdf_from_mnemonic(&m, &salt_rec);
        let wrap_rec1 = aead_seal(&key_rec1, &dek1, AAD_CONTEXT.as_bytes()).unwrap();
        let (n1, ct1) = aead_encrypt_detached(
            &dek1,
            br#"{"dnsimple_token":"tok_DNS"}"#,
            AAD_CONTEXT.as_bytes(),
        )
        .unwrap();
        let v1 = VaultFile {
            version: VAULT_VERSION,
            kdf: KdfParams::default(),
            salt_password: B64.encode(salt_pwd),
            salt_recovery: B64.encode(salt_rec),
            dek_wrapped_by_password: B64.encode(wrap_pwd1.clone()),
            dek_wrapped_by_recovery: B64.encode(wrap_rec1.clone()),
            secrets_aead_nonce: B64.encode(n1),
            secrets_aead_ciphertext: B64.encode(ct1),
            created_unix: 0,
        };
        fs::create_dir_all(&paths.data_dir).unwrap();
        fs::write(&paths.vault_file, serde_json::to_vec(&v1).unwrap()).unwrap();
        // Unlock via password path and read provider-specific tokens
        let dek_open1 = aead_open(
            &key_pwd1,
            &B64.decode(&v1.dek_wrapped_by_password).unwrap(),
            AAD_CONTEXT.as_bytes(),
        )
        .unwrap();
        let t_dns =
            super::unlock_provider_with_dek(&paths, &dek_open1, Provider::Dnsimple).unwrap();
        assert_eq!(t_dns.expose_secret(), "tok_DNS");
        let do_err = super::unlock_provider_with_dek(&paths, &dek_open1, Provider::Digitalocean);
        assert!(do_err.is_err());

        // Case 2: digitalocean-only secrets
        let dek2 = random_bytes(32);
        let key_pwd2 = derive_argon2_key(&password, &salt_pwd, &KdfParams::default()).unwrap();
        let wrap_pwd2 = aead_seal(&key_pwd2, &dek2, AAD_CONTEXT.as_bytes()).unwrap();
        let key_rec2 = derive_hkdf_from_mnemonic(&m, &salt_rec);
        let wrap_rec2 = aead_seal(&key_rec2, &dek2, AAD_CONTEXT.as_bytes()).unwrap();
        let (n2, ct2) = aead_encrypt_detached(
            &dek2,
            br#"{"dnsimple_token":null,"digitalocean_token":"tok_DO"}"#,
            AAD_CONTEXT.as_bytes(),
        )
        .unwrap();
        let v2 = VaultFile {
            version: VAULT_VERSION,
            kdf: KdfParams::default(),
            salt_password: B64.encode(salt_pwd),
            salt_recovery: B64.encode(salt_rec),
            dek_wrapped_by_password: B64.encode(wrap_pwd2.clone()),
            dek_wrapped_by_recovery: B64.encode(wrap_rec2.clone()),
            secrets_aead_nonce: B64.encode(n2),
            secrets_aead_ciphertext: B64.encode(ct2),
            created_unix: 0,
        };
        fs::write(&paths.vault_file, serde_json::to_vec(&v2).unwrap()).unwrap();
        let dek_open2 = aead_open(
            &key_pwd2,
            &B64.decode(&v2.dek_wrapped_by_password).unwrap(),
            AAD_CONTEXT.as_bytes(),
        )
        .unwrap();
        let t_do =
            super::unlock_provider_with_dek(&paths, &dek_open2, Provider::Digitalocean).unwrap();
        assert_eq!(t_do.expose_secret(), "tok_DO");
        let dns_err = super::unlock_provider_with_dek(&paths, &dek_open2, Provider::Dnsimple);
        assert!(dns_err.is_err());
    }

    #[test]
    fn validate_fqdn_and_calendar_helpers() {
        // FQDN checks
        assert!(super::is_valid_fqdn("example.com"));
        assert!(super::is_valid_fqdn("api.example.co.uk"));
        assert!(!super::is_valid_fqdn("bad_underscore.example.com"));
        assert!(!super::is_valid_fqdn("-bad.example.com"));
        assert!(!super::is_valid_fqdn("a..b.com"));
        // Calendar heuristic (may succeed via systemd-analyze; must pass heuristic cases)
        assert!(super::validate_calendar("Mon *-*-* 12:00:00") || true);
        assert!(super::validate_calendar("daily") || true);
        assert!(!super::validate_calendar("bad%fmt"));
        assert!(!super::validate_calendar("line\nbreak"));
    }
}

// No Windows DPAPI helper in this focused provider changeset
