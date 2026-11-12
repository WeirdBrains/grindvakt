//! Paths and the config file. The config format itself lives in `grindvakt_core::config`.
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use crate::fsutil::{PRIVACY, ensure_dir_0700};

pub use grindvakt_core::config::{AppConfig, Provider, Schedule, normalize_fqdn};

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
        let config_file = data_dir.join(grindvakt_core::config::CONFIG_FILE_NAME);
        let vault_file = data_dir.join(grindvakt_core::vault::VAULT_FILE_NAME);
        let credstore_dir = default_credstore_dir()?;
        Ok(Self {
            data_dir,
            config_file,
            vault_file,
            credstore_dir,
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn home_dir() -> Result<PathBuf> {
    let user_dirs = directories::UserDirs::new().context("could not resolve home directory")?;
    Ok(user_dirs.home_dir().to_path_buf())
}

#[cfg(target_os = "windows")]
fn program_data_dir() -> Result<PathBuf> {
    // Prefer %PROGRAMDATA% when available; fall back to C:\ProgramData.
    if let Ok(pd) = std::env::var("PROGRAMDATA") {
        Ok(PathBuf::from(pd))
    } else {
        Ok(PathBuf::from(r"C:\ProgramData"))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn default_data_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".grindvakt"))
}

#[cfg(target_os = "windows")]
fn default_data_dir() -> Result<PathBuf> {
    Ok(program_data_dir()?.join("Grindvakt"))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn default_credstore_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".config/systemd/user/credstore.encrypted"))
}

#[cfg(target_os = "windows")]
fn default_credstore_dir() -> Result<PathBuf> {
    // Shared credentials directory under ProgramData; readable by SYSTEM, Administrators and NT SERVICE\Grindvakt only.
    Ok(program_data_dir()?.join("Grindvakt").join("credentials"))
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
    Ok(AppConfig::load(path)?)
}

pub fn write_config_atomic(paths: &ConfigPaths, cfg: &AppConfig) -> Result<()> {
    cfg.save_with(&paths.config_file, PRIVACY)
        .context("write config")?;
    Ok(())
}
