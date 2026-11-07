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
use rpassword::prompt_password;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fs;
use std::io::{Write as IoWrite, stdin, stdout};
use std::path::Path;

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

#[derive(Clone, Serialize, Deserialize)]
struct Secrets {
    dnsimple_token: String,
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

        println!(
            "Config to be saved at: {}",
            self.paths.config_file.display()
        );
        println!("Zone: {}", self.cfg.zone);
        println!("Name: {}", self.cfg.name);
        println!("TTL: {}", self.cfg.ttl);
        println!("Sandbox: {}", self.cfg.sandbox);

        // Prompt for token and password
        let token = prompt_secret("Enter DNSimple API token: ")?;
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

        // Encrypt secrets with DEK
        let secrets = Secrets {
            dnsimple_token: token.expose_secret().clone(),
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
        write_config_atomic(self.paths, &self.cfg)?;
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
    Ok(SecretString::new(s.dnsimple_token))
}

pub struct CredentialEnrollment<'a> {
    paths: &'a ConfigPaths,
}
impl<'a> CredentialEnrollment<'a> {
    pub fn new(paths: &'a ConfigPaths) -> Self {
        Self { paths }
    }
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
    eprint!("{}", prompt);
    let s = prompt_password(prompt)
        .map_err(|e| anyhow::anyhow!("tty required for secret input: {}", e))?;
    Ok(SecretString::new(s))
}

fn prompt_line(prompt: &str) -> Result<String> {
    eprint!("{}", prompt);
    stdout().flush().ok();
    let mut s = String::new();
    stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
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
}
