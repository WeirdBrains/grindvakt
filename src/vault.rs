//! Interactive vault flows and unattended credentials.
//!
//! The vault format, its crypto (Argon2id, 24-word recovery, XChaCha20-Poly1305) and its
//! on-disk layout come from `grindvakt_core::vault`; this module adds the prompts and the
//! OS credential stores (systemd-creds with TPM2, the macOS Keychain, Windows DPAPI) that
//! hold the data key for unattended runs.

use crate::config::{AppConfig, ConfigPaths, Provider, write_config_atomic};
#[cfg(target_os = "windows")]
use crate::fsutil::atomic_write_0600;
use crate::fsutil::{PRIVACY, ensure_dir_0700};
use crate::schedule::validate_calendar;
use anyhow::{Context, Result, anyhow, bail};
use grindvakt_core::config::validate::{is_valid_fqdn, is_valid_record_name};
use grindvakt_core::vault::{Dek, KdfParams, RecoveryPhrase, Tokens, Vault};
use rpassword::prompt_password;
use secrecy::{ExposeSecret, SecretString};
use std::io::{Write as IoWrite, stdin, stdout};
use std::path::Path;

pub use grindvakt_core::vault::{TokenStatus, VaultFile};

const CREDENTIAL_NAME: &str = "grindvakt.dek";

#[cfg(target_os = "macos")]
fn keychain_identity_for(paths: &ConfigPaths) -> (String, String) {
    // Vault-scoped account id to avoid cross-vault overwrite: account = "grindvakt.dek:<12-hex>"
    let service = "grindvakt".to_string();
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let real = std::fs::canonicalize(&paths.data_dir).unwrap_or_else(|_| paths.data_dir.clone());
    hasher.update(real.display().to_string().as_bytes());
    let digest = hasher.finalize();
    let mut suffix = String::with_capacity(12);
    for b in digest[..6].iter() {
        use std::fmt::Write as _;
        let _ = write!(&mut suffix, "{:02x}", b);
    }
    let account = format!("{}:{}", CREDENTIAL_NAME, suffix);
    (service, account)
}

#[cfg(target_os = "macos")]
fn try_unattended_keychain_dek_scoped(
    paths: &ConfigPaths,
    keychains: Option<&[security_framework::os::macos::keychain::SecKeychain]>,
) -> Result<Option<Vec<u8>>> {
    use security_framework::os::macos::passwords::find_generic_password;
    const ERR_ITEM_NOT_FOUND: i32 = -25300; // errSecItemNotFound
    let (svc, acct) = keychain_identity_for(paths);
    match find_generic_password(keychains, &svc, &acct) {
        Ok((pwd, _item)) => {
            let bytes = pwd.as_ref();
            if bytes.len() != 32 {
                bail!(
                    "keychain entry has invalid length {}; expected 32 — re-enroll with `grindvakt service enroll-credential`",
                    bytes.len()
                );
            }
            Ok(Some(bytes.to_vec()))
        }
        Err(e) => {
            if e.code() == ERR_ITEM_NOT_FOUND {
                // Try legacy global identity (read-only compatibility)
                match find_generic_password(keychains, "grindvakt", "grindvakt.dek") {
                    Ok((pwd, _item)) => {
                        let bytes = pwd.as_ref();
                        if bytes.len() != 32 {
                            bail!(
                                "legacy keychain entry has invalid length {}; expected 32 — re-enroll with `grindvakt service enroll-credential`",
                                bytes.len()
                            );
                        }
                        return Ok(Some(bytes.to_vec()));
                    }
                    Err(e2) => {
                        if e2.code() == ERR_ITEM_NOT_FOUND {
                            return Ok(None);
                        }
                        return Err(anyhow::anyhow!("keychain lookup failed: {}", e2));
                    }
                }
            }
            Err(anyhow::anyhow!("keychain lookup failed: {}", e))
        }
    }
}

#[cfg(target_os = "macos")]
fn store_unattended_keychain_dek_scoped(
    paths: &ConfigPaths,
    dek: &[u8],
    keychain: Option<&security_framework::os::macos::keychain::SecKeychain>,
) -> Result<()> {
    let (svc, acct) = keychain_identity_for(paths);
    if let Some(kc) = keychain {
        kc.set_generic_password(&svc, &acct, dek)
            .context("store DEK in Keychain")?;
    } else {
        security_framework::passwords::set_generic_password(&svc, &acct, dek)
            .context("store DEK in Keychain")?;
    }
    Ok(())
}

fn load_vault(path: &Path) -> Result<Vault> {
    Vault::load(path).with_context(|| format!("load vault {}", path.display()))
}

fn save_vault(path: &Path, vault: &Vault) -> Result<()> {
    vault.save_with(path, PRIVACY).context("write vault")
}

/// Prompts for the password and unwraps the data key.
fn unlock_with_prompt(vault: &Vault, prompt: &str) -> Result<Dek> {
    let pwd = prompt_secret(prompt)?;
    vault
        .unlock_with_password(&pwd)
        .map_err(|e| anyhow!("unlock failed: {e}"))
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

        // 24-word recovery phrase from 256 bits of OS randomness.
        let phrase = RecoveryPhrase::generate()?;
        println!("\nYour 24-word recovery phrase (write it down, never store it digitally):\n");
        println!("{}", phrase.expose_words());
        println!("\nYou will be asked to re-enter the phrase to confirm.");
        let check = prompt_line("Re-enter recovery phrase: ")?;
        if !phrase.matches(&check) {
            bail!("recovery phrase did not match");
        }

        let mut tokens = Tokens::default();
        tokens.set(cfg.provider, token);
        let vault = Vault::create_with_phrase(&password, &phrase, &tokens, KdfParams::default())?;

        // Write config and vault atomically
        write_config_atomic(self.paths, &cfg)?;
        save_vault(&self.paths.vault_file, &vault)?;
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
        let mut vault = load_vault(&self.paths.vault_file)?;
        println!("Enter your 24-word recovery phrase to rewrap the password.");
        let phrase_str = prompt_line("Recovery phrase: ")?;
        let phrase =
            RecoveryPhrase::parse(&phrase_str).map_err(|_| anyhow!("invalid recovery phrase"))?;
        let dek = vault
            .unlock_with_recovery(&phrase)
            .map_err(|e| anyhow!("recovery unlock failed: {e}"))?;
        let new_pwd = prompt_secret("New password: ")?;
        let new_pwd2 = prompt_secret("Confirm new password: ")?;
        if new_pwd.expose_secret() != new_pwd2.expose_secret() {
            bail!("passwords do not match");
        }
        vault.set_password(&dek, &new_pwd)?;
        save_vault(&self.paths.vault_file, &vault)?;
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
        let mut vault = load_vault(&self.paths.vault_file)?;
        let current = prompt_secret("Current password: ")?;
        let dek = vault
            .unlock_with_password(&current)
            .map_err(|_| anyhow!("password incorrect or vault tampered"))?;
        let new_pwd = prompt_secret("New password: ")?;
        let new_pwd2 = prompt_secret("Confirm new password: ")?;
        if new_pwd.expose_secret() != new_pwd2.expose_secret() {
            bail!("passwords do not match");
        }
        vault.set_password(&dek, &new_pwd)?;
        save_vault(&self.paths.vault_file, &vault)?;
        println!("Password updated.");
        Ok(())
    }
}

pub struct UnlockContext<'a> {
    pub paths: &'a ConfigPaths,
    pub allow_unattended: bool,
}

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
    #[cfg(target_os = "windows")]
    if ctx.allow_unattended {
        let pd = std::env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".to_string());
        let p = std::path::Path::new(&pd)
            .join("Grindvakt")
            .join("credentials")
            .join("grindvakt.dek.dpapi");
        if p.exists() {
            match std::fs::read(&p) {
                Ok(blob) => match crate::winutil::dpapi_unprotect(&blob) {
                    Ok(dek) if dek.len() == 32 => {
                        return unlock_provider_with_dek(ctx.paths, &dek, provider);
                    }
                    Err(e) => {
                        eprintln!(
                            "Unattended credential present but DPAPI unlock failed: {}",
                            e
                        );
                    }
                    _ => {}
                },
                Err(e) => {
                    eprintln!("Failed to read unattended credential: {}", e);
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    if ctx.allow_unattended
        && let Some(data) = try_unattended_keychain_dek_scoped(ctx.paths, None)?
    {
        return unlock_provider_with_dek(ctx.paths, &data, provider);
    }
    // If unattended was requested under launchd (no TTY), return actionable guidance instead of attempting a prompt.
    #[cfg(target_os = "macos")]
    if ctx.allow_unattended && !stdin_is_tty() {
        bail!(
            "unattended credential not found in Keychain; run `grindvakt service enroll-credential`"
        );
    }
    let vault = load_vault(&ctx.paths.vault_file)?;
    let dek = unlock_with_prompt(&vault, "Vault password: ")?;
    token_from(&vault, &dek, provider)
}

fn token_from(vault: &Vault, dek: &Dek, provider: Provider) -> Result<SecretString> {
    match vault.token(dek, provider) {
        Ok(t) => Ok(t),
        Err(grindvakt_core::Error::TokenMissing(p)) => {
            bail!("{p} token not set; run `grindvakt provider token set {p}`")
        }
        Err(e) => Err(e.into()),
    }
}

/// Decrypts `provider`'s token with a data key released by an OS credential store.
pub fn unlock_provider_with_dek(
    paths: &ConfigPaths,
    dek: &[u8],
    provider: Provider,
) -> Result<SecretString> {
    let vault = load_vault(&paths.vault_file)?;
    let dek = Dek::from_bytes(dek)?;
    vault.verify_dek(&dek).map_err(|e| {
        anyhow!(
            "decrypt secrets: {e} (stale/wrong unattended credential?). Rerun `grindvakt service enroll-credential`."
        )
    })?;
    token_from(&vault, &dek, provider)
}

pub async fn tokens_status_interactive(paths: &ConfigPaths) -> Result<TokenStatus> {
    let vault = load_vault(&paths.vault_file)?;
    let dek = unlock_with_prompt(&vault, "Vault password: ")?;
    Ok(vault.token_status(&dek)?)
}

pub async fn set_provider_token_interactive(paths: &ConfigPaths, provider: Provider) -> Result<()> {
    let mut vault = load_vault(&paths.vault_file)?;
    let dek = unlock_with_prompt(&vault, "Vault password: ")?;
    let label = match provider {
        Provider::Dnsimple => "Enter DNSimple API token: ",
        Provider::Digitalocean => "Enter DigitalOcean API token: ",
    };
    let tok = prompt_secret(label)?;
    vault.set_token(&dek, provider, tok)?;
    save_vault(&paths.vault_file, &vault)?;
    println!("Token updated for {provider}");
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
        let dek = unlock_with_prompt(&vault, "Vault password (to access DEK): ")?;
        // Pipe DEK to systemd-creds --user encrypt --with-key=host+tpm2
        let dest_dir = &self.paths.credstore_dir;
        ensure_dir_0700(dest_dir)?;
        let dest_file = dest_dir.join(format!("{}.enc", CREDENTIAL_NAME));
        // Build argument list once, so tests can validate without spawning
        let args = systemd_creds_encrypt_args(&dest_file);
        let mut child = {
            let mut cmd = tokio::process::Command::new("systemd-creds");
            cmd.args(&args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped());
            cmd.spawn().context("launch systemd-creds")?
        };
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin.write_all(dek.as_bytes()).await?;
        }
        let output = child.wait_with_output().await?;
        drop(dek); // the key is wiped on drop
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
        let dek = unlock_with_prompt(&vault, "Vault password (to access DEK): ")?;
        // Store using default keychain in production
        store_unattended_keychain_dek_scoped(self.paths, dek.as_bytes(), None)?;
        let (svc, acct) = keychain_identity_for(self.paths);
        println!(
            "Enrolled unattended credential in Keychain (service={}, account={})",
            svc, acct
        );
        Ok(())
    }
    #[cfg(target_os = "windows")]
    pub async fn enroll(&self, _cfg: &AppConfig) -> Result<()> {
        let vault = load_vault(&self.paths.vault_file)?;
        let dek = unlock_with_prompt(&vault, "Vault password (to access DEK): ")?;
        let blob = crate::winutil::dpapi_protect_machine_scope(dek.as_bytes())?;
        let pd = std::env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".to_string());
        let cred_dir = std::path::Path::new(&pd)
            .join("Grindvakt")
            .join("credentials");
        ensure_dir_0700(&cred_dir)?;
        let dest = cred_dir.join("grindvakt.dek.dpapi");
        atomic_write_0600(&dest, &blob).context("write dpapi blob")?;
        println!(
            "Enrolled unattended credential at {} (DPAPI machine scope)",
            dest.display()
        );
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    pub async fn enroll(&self, _cfg: &AppConfig) -> Result<()> {
        bail!("unattended credential enrollment unsupported on this OS");
    }
}

#[cfg(target_os = "macos")]
fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

#[cfg(target_os = "linux")]
fn systemd_creds_encrypt_args(dest_file: &std::path::Path) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;
    vec![
        OsString::from("--user"),
        OsString::from("encrypt"),
        OsString::from("--name"),
        OsString::from(CREDENTIAL_NAME),
        OsString::from("--with-key=host+tpm2"),
        OsString::from("-"),
        dest_file.as_os_str().to_os_string(),
    ]
}

fn prompt_secret(prompt: &str) -> Result<SecretString> {
    // rpassword::prompt_password renders the prompt; avoid double-printing.
    let s = prompt_password(prompt)
        .map_err(|e| anyhow::anyhow!("tty required for secret input: {}", e))?;
    Ok(SecretString::from(s))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn paths_in(base: &Path) -> ConfigPaths {
        ConfigPaths {
            data_dir: base.join("data"),
            config_file: base.join("data/config.toml"),
            vault_file: base.join("data/vault.json"),
            credstore_dir: base.join("credstore.encrypted"),
        }
    }

    /// Writes a vault with fast KDF settings straight to disk (no privacy checks, so this
    /// works in any temp dir on every OS) and returns its data key.
    fn write_vault(paths: &ConfigPaths, tokens: &Tokens) -> Dek {
        fs::create_dir_all(&paths.data_dir).unwrap();
        let pwd = SecretString::from("pw".to_string());
        let (vault, _phrase) = Vault::create_with_kdf(&pwd, tokens, KdfParams::testing()).unwrap();
        fs::write(&paths.vault_file, vault.to_json()).unwrap();
        vault.unlock_with_password(&pwd).unwrap()
    }

    fn one(provider: Provider, token: &str) -> Tokens {
        let mut t = Tokens::default();
        t.set(provider, SecretString::from(token.to_string()));
        t
    }

    #[test]
    fn unattended_dek_unlocks_the_providers_token() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        let dek = write_vault(&paths, &one(Provider::Dnsimple, "tok_DNS"));
        let t = unlock_provider_with_dek(&paths, dek.as_bytes(), Provider::Dnsimple).unwrap();
        assert_eq!(t.expose_secret(), "tok_DNS");
        let err = unlock_provider_with_dek(&paths, dek.as_bytes(), Provider::Digitalocean)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(
                "digitalocean token not set; run `grindvakt provider token set digitalocean`"
            ),
            "{err}"
        );
    }

    #[test]
    fn digitalocean_only_vault() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        let dek = write_vault(&paths, &one(Provider::Digitalocean, "tok_DO"));
        let t = unlock_provider_with_dek(&paths, dek.as_bytes(), Provider::Digitalocean).unwrap();
        assert_eq!(t.expose_secret(), "tok_DO");
        assert!(unlock_provider_with_dek(&paths, dek.as_bytes(), Provider::Dnsimple).is_err());
    }

    #[test]
    fn stale_or_malformed_unattended_dek_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        write_vault(&paths, &one(Provider::Dnsimple, "tok"));
        let err = unlock_provider_with_dek(&paths, &[7u8; 32], Provider::Dnsimple)
            .unwrap_err()
            .to_string();
        assert!(err.contains("stale/wrong unattended credential"), "{err}");
        assert!(unlock_provider_with_dek(&paths, b"short", Provider::Dnsimple).is_err());
    }

    #[test]
    fn password_and_recovery_flows_rewrap_the_same_key() {
        let pwd = SecretString::from("old".to_string());
        let new = SecretString::from("new".to_string());
        let (mut vault, phrase) =
            Vault::create_with_kdf(&pwd, &one(Provider::Dnsimple, "tok"), KdfParams::testing())
                .unwrap();
        let dek = vault.unlock_with_password(&pwd).unwrap();
        assert!(vault.unlock_with_password(&new).is_err());
        vault.set_password(&dek, &new).unwrap();
        assert!(vault.unlock_with_password(&pwd).is_err());
        let again = vault.unlock_with_recovery(&phrase).unwrap();
        assert_eq!(dek.as_bytes(), again.as_bytes());
        assert!(phrase.matches(&phrase.expose_words().to_uppercase()));
        assert!(RecoveryPhrase::parse("not a phrase").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_identity_derivation_scoped_stable() {
        let base = TempDir::new().unwrap();
        let p1 = paths_in(&base.path().join("one"));
        let p2 = paths_in(&base.path().join("two"));
        let (svc1, acct1) = keychain_identity_for(&p1);
        let (svc2, acct2) = keychain_identity_for(&p2);
        assert_eq!(svc1, "grindvakt");
        assert_eq!(svc2, "grindvakt");
        assert_ne!(acct1, acct2);
        assert!(acct1.starts_with("grindvakt.dek:"));
        assert_eq!(keychain_identity_for(&p1), (svc1, acct1));
    }

    #[cfg(target_os = "macos")]
    fn test_keychain(base: &Path) -> security_framework::os::macos::keychain::SecKeychain {
        security_framework::os::macos::keychain::CreateOptions::new()
            .password("pw")
            .create(base.join("Test.keychain-db"))
            .expect("create keychain")
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_keychain_isolated_enroll_lookup_and_decrypt() {
        let tmp = TempDir::new().unwrap();
        let kc = test_keychain(tmp.path());
        let paths = paths_in(tmp.path());
        let dek = write_vault(&paths, &one(Provider::Dnsimple, "tok_X"));
        store_unattended_keychain_dek_scoped(&paths, dek.as_bytes(), Some(&kc)).unwrap();
        let found = try_unattended_keychain_dek_scoped(&paths, Some(&[kc])).unwrap();
        let token = unlock_provider_with_dek(&paths, &found.unwrap(), Provider::Dnsimple).unwrap();
        assert_eq!(token.expose_secret(), "tok_X");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_keychain_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let kc = test_keychain(tmp.path());
        let paths = paths_in(tmp.path());
        assert!(
            try_unattended_keychain_dek_scoped(&paths, Some(&[kc]))
                .unwrap()
                .is_none()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_keychain_wrong_length_is_error() {
        let tmp = TempDir::new().unwrap();
        let kc = test_keychain(tmp.path());
        let paths = paths_in(tmp.path());
        store_unattended_keychain_dek_scoped(&paths, b"short", Some(&kc)).unwrap();
        let err = try_unattended_keychain_dek_scoped(&paths, Some(&[kc])).unwrap_err();
        assert!(err.to_string().contains("invalid length"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_creds_encrypt_args_include_explicit_credential_name() {
        use std::ffi::OsString;
        let dest = Path::new("/tmp/credstore.encrypted/grindvakt.dek.enc");
        let got = super::systemd_creds_encrypt_args(dest);
        let want: Vec<OsString> = vec![
            "--user".into(),
            "encrypt".into(),
            "--name".into(),
            CREDENTIAL_NAME.into(),
            "--with-key=host+tpm2".into(),
            "-".into(),
            dest.as_os_str().to_os_string(),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn validate_fqdn_and_calendar_helpers() {
        assert!(is_valid_fqdn("example.com"));
        assert!(is_valid_fqdn("api.example.co.uk"));
        assert!(!is_valid_fqdn("bad_underscore.example.com"));
        assert!(!is_valid_fqdn("-bad.example.com"));
        assert!(!is_valid_fqdn("a..b.com"));
        assert!(is_valid_record_name("example.com", "@"));
        assert!(is_valid_record_name("example.com", "home.office"));
        assert!(!validate_calendar("bad%fmt"));
        assert!(!validate_calendar("line\nbreak"));
    }
}
