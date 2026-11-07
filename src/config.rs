use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::fsutil::{atomic_write_0600, ensure_dir_0700};

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub data_dir: PathBuf,      // ~/.grindvakt
    pub config_file: PathBuf,   // ~/.grindvakt/config.toml
    pub vault_file: PathBuf,    // ~/.grindvakt/vault.json
    pub credstore_dir: PathBuf, // ~/.config/systemd/user/credstore.encrypted
}

impl ConfigPaths {
    pub fn resolve(override_dir: Option<&Path>) -> Result<Self> {
        let data_dir = if let Some(dir) = override_dir {
            dir.to_path_buf()
        } else {
            default_data_dir()?
        };
        let config_file = data_dir.join("config.toml");
        let vault_file = data_dir.join("vault.json");
        let credstore_dir = default_credstore_dir()?;
        Ok(Self {
            data_dir,
            config_file,
            vault_file,
            credstore_dir,
        })
    }
}

fn home_dir() -> Result<PathBuf> {
    let user_dirs = UserDirs::new().context("could not resolve home directory")?;
    Ok(user_dirs.home_dir().to_path_buf())
}

fn default_data_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".grindvakt"))
}
fn default_credstore_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".config/systemd/user/credstore.encrypted"))
}

pub fn ensure_data_dir(paths: &ConfigPaths) -> Result<()> {
    ensure_dir_0700(&paths.data_dir)?;
    Ok(())
}

pub fn load_or_init_config(paths: &ConfigPaths) -> Result<AppConfig> {
    if !paths.config_file.exists() {
        bail!(
            "Config not found at {}. Run `grindvakt init` first or create it.",
            paths.config_file.display()
        );
    }
    read_config(&paths.config_file)
}

pub fn read_config(path: &Path) -> Result<AppConfig> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let cfg: AppConfig =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    cfg.validate()?;
    Ok(cfg)
}

pub fn write_config_atomic(paths: &ConfigPaths, cfg: &AppConfig) -> Result<()> {
    let text = toml::to_string_pretty(cfg).context("serialize config")?;
    atomic_write_0600(&paths.config_file, text.as_bytes()).context("write config")?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Dnsimple,
    Digitalocean,
}
fn default_provider() -> Provider {
    Provider::Dnsimple
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provider::Dnsimple => write!(f, "dnsimple"),
            Provider::Digitalocean => write!(f, "digitalocean"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    /// Optional: systemd OnCalendar expression
    #[serde(default)]
    pub calendar: Option<String>,
    /// Optional: polling interval in seconds (OnUnitActiveSec)
    #[serde(default)]
    pub interval_secs: Option<u64>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            calendar: None,
            interval_secs: Some(300),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Active DNS provider
    #[serde(default = "default_provider")]
    pub provider: Provider,
    /// DNSimple account id (whoami preferred when using account token)
    #[serde(default)]
    pub account_id: Option<u64>,
    /// DNS zone (e.g., example.com)
    pub zone: String,
    /// Record name: '@' for apex, relative label like 'www', or FQDN
    pub name: String,
    /// TTL in seconds (min 60, default 300)
    #[serde(default = "default_ttl")]
    pub ttl: u32,
    /// Sandbox API (uses DNSimple sandbox)
    #[serde(default)]
    pub sandbox: bool,
    /// Optional API base_url override (for tests)
    #[serde(default)]
    pub api_base_url: Option<String>,
    /// Schedule for systemd timer
    #[serde(default)]
    pub schedule: Schedule,
}

fn default_ttl() -> u32 {
    300
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            account_id: None,
            zone: "example.com".to_string(),
            name: "@".to_string(),
            ttl: default_ttl(),
            sandbox: false,
            api_base_url: None,
            schedule: Schedule::default(),
        }
    }
}

impl AppConfig {
    pub fn validate(&self) -> Result<()> {
        if self.ttl < 60 {
            bail!("ttl must be >= 60");
        }
        if self.zone.trim().is_empty() {
            bail!("zone must be set");
        }
        if self.name.trim().is_empty() {
            bail!("name must be set");
        }
        if self.schedule.calendar.is_none() && self.schedule.interval_secs.is_none() {
            bail!("schedule must set either calendar or interval_secs");
        }
        Ok(())
    }
    pub fn record_fqdn(&self) -> String {
        normalize_fqdn(&self.zone, &self.name)
    }
}

pub fn normalize_fqdn(zone: &str, name: &str) -> String {
    let z = zone.trim_end_matches('.').to_string();
    let n = name.trim().trim_end_matches('.');
    if n == "@" || n.is_empty() {
        z
    } else if n.ends_with(&z) {
        n.to_string()
    } else {
        format!("{}.", n) + &z
    }
}
