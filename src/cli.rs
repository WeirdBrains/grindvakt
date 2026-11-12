use crate::config::{self, AppConfig, ConfigPaths, Provider};
use crate::ip::PublicIpConsensus;
use crate::service::InstallUnits;
use crate::sync::{SyncMode, sync_record};
use crate::vault::{
    ChangePasswordFlow, CredentialEnrollment, RecoverFlow, VaultInitFlow,
    set_provider_token_interactive, tokens_status_interactive,
    unlock_token_for_provider_interactive,
};
use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Parser, Subcommand};
use std::path::PathBuf;
use tracing::info;

#[derive(Parser, Debug)]
#[command(
    name = "grindvakt",
    version,
    about = "User-mode DNS updater for DNSimple"
)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Manage active DNS provider and tokens
    Provider {
        #[command(subcommand)]
        cmd: ProviderCmd,
    },
    /// Interactively create initial config and encrypted vault
    Init(Init),
    /// Sync A record to current public IPv4 (idempotent). Never writes with --dry-run
    Sync(Sync),
    /// Use 24-word recovery phrase to reset password (rewrap DEK, no token change)
    Recover(Recover),
    /// Change password using current password (rewrap DEK)
    ChangePassword(ChangePassword),
    #[cfg_attr(
        target_os = "linux",
        doc = "Manage systemd --user units and unattended credential"
    )]
    #[cfg_attr(
        target_os = "macos",
        doc = "Manage launchd per-user agent and Keychain unattended credential"
    )]
    #[cfg_attr(
        target_os = "windows",
        doc = "Manage Windows service (SCM) and unattended credential"
    )]
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
}

#[derive(ClapArgs, Debug)]
pub struct Common {
    /// Override data dir (defaults to ~/.grindvakt)
    #[arg(long, env = "GRINDVAKT_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// Increase output verbosity
    #[arg(long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

#[derive(ClapArgs, Debug)]
pub struct Init {
    #[command(flatten)]
    pub common: Common,
    /// Optional: pre-seed config from this file (TOML); prompts will confirm
    #[arg(long)]
    pub from: Option<PathBuf>,
    /// Sandbox API (dnsimple sandbox)
    #[arg(long)]
    pub sandbox: bool,
}

impl Init {
    pub async fn run(self) -> Result<()> {
        let paths = ConfigPaths::resolve(self.common.data_dir.as_deref())?;
        config::ensure_data_dir(&paths)?;
        let mut cfg = if let Some(p) = self.from {
            config::read_config(&p).context("failed to read seed config")?
        } else {
            AppConfig::default()
        };
        if self.sandbox {
            cfg.sandbox = true;
        }

        VaultInitFlow::new(&paths, cfg).run_interactive().await
    }
}

#[derive(ClapArgs, Debug)]
pub struct Sync {
    #[command(flatten)]
    pub common: Common,
    /// Do not perform API writes; print intended changes
    #[arg(long)]
    pub dry_run: bool,
    /// Force using only password-unlocked vault; fail if unattended credential is required/missing
    #[arg(long)]
    pub no_unattended: bool,
}

impl Sync {
    pub async fn run(self) -> Result<()> {
        let paths = ConfigPaths::resolve(self.common.data_dir.as_deref())?;
        let cfg = config::load_or_init_config(&paths).context("load config")?;
        // Prevent concurrent runs (service/manual overlap)
        let _lock =
            crate::lock::SingleInstanceLock::acquire(&format!("sync-{}", cfg.record_fqdn()))?;

        let ctx = crate::vault::UnlockContext {
            paths: &paths,
            allow_unattended: !self.no_unattended,
        };
        let token = unlock_token_for_provider_interactive(ctx, cfg.provider).await?;
        // Check the provider settings before asking the network for the public IP.
        crate::provider::build_active_provider(&cfg, &token)?;
        let consensus = PublicIpConsensus::default();
        let ip = consensus.resolve().await.context("resolve public IPv4")?;
        let mode = if self.dry_run {
            SyncMode::DryRun
        } else {
            SyncMode::Apply
        };
        let outcome = sync_record(&cfg, &token, ip, mode)
            .await
            .context("sync dns record")?;
        info!("{outcome}");
        Ok(())
    }
}

#[derive(Subcommand, Debug)]
pub enum ProviderCmd {
    /// Show current active provider
    Current {
        #[command(flatten)]
        common: Common,
    },
    /// Set the active provider
    Set {
        #[command(flatten)]
        common: Common,
        /// dnsimple or digitalocean
        to: Provider,
    },
    /// List supported providers and token status (prompts to unlock vault)
    List {
        #[command(flatten)]
        common: Common,
    },
    /// Manage provider tokens
    Token {
        #[command(subcommand)]
        cmd: ProviderTokenCmd,
    },
}

#[derive(Subcommand, Debug)]
pub enum ProviderTokenCmd {
    /// Set or replace the token for a provider (stored encrypted in the vault)
    Set {
        #[command(flatten)]
        common: Common,
        /// dnsimple or digitalocean
        provider: Provider,
    },
}

impl ProviderCmd {
    pub async fn run(self) -> Result<()> {
        match self {
            ProviderCmd::Current { common } => {
                let paths = ConfigPaths::resolve(common.data_dir.as_deref())?;
                let cfg = config::load_or_init_config(&paths)?;
                println!("{}", cfg.provider);
                Ok(())
            }
            ProviderCmd::Set { common, to } => {
                let paths = ConfigPaths::resolve(common.data_dir.as_deref())?;
                let mut cfg = config::load_or_init_config(&paths)?;
                cfg.provider = to;
                config::write_config_atomic(&paths, &cfg)?;
                println!("Active provider set to {}", cfg.provider);
                Ok(())
            }
            ProviderCmd::List { common } => {
                let paths = ConfigPaths::resolve(common.data_dir.as_deref())?;
                let status = tokens_status_interactive(&paths).await?;
                let cur = config::load_or_init_config(&paths)?.provider;
                println!(
                    "supported: dnsimple, digitalocean
current: {}
tokens: dnsimple = {}, digitalocean = {}",
                    cur,
                    if status.dnsimple { "set" } else { "missing" },
                    if status.digitalocean {
                        "set"
                    } else {
                        "missing"
                    }
                );
                Ok(())
            }
            ProviderCmd::Token { cmd } => match cmd {
                ProviderTokenCmd::Set { common, provider } => {
                    let paths = ConfigPaths::resolve(common.data_dir.as_deref())?;
                    set_provider_token_interactive(&paths, provider).await
                }
            },
        }
    }
}
#[derive(ClapArgs, Debug)]
pub struct Recover {
    #[command(flatten)]
    pub common: Common,
}
impl Recover {
    pub async fn run(self) -> Result<()> {
        let paths = ConfigPaths::resolve(self.common.data_dir.as_deref())?;
        RecoverFlow::new(&paths).run_interactive().await
    }
}

#[derive(ClapArgs, Debug)]
pub struct ChangePassword {
    #[command(flatten)]
    pub common: Common,
}
impl ChangePassword {
    pub async fn run(self) -> Result<()> {
        let paths = ConfigPaths::resolve(self.common.data_dir.as_deref())?;
        ChangePasswordFlow::new(&paths).run_interactive().await
    }
}

#[derive(Subcommand, Debug)]
pub enum ServiceCmd {
    /// Print the service and timer units to stdout
    Print(PrintService),
    #[cfg_attr(
        target_os = "linux",
        doc = "Install service and timer units in ~/.config/systemd/user (not enabled)"
    )]
    #[cfg_attr(
        target_os = "macos",
        doc = "Install per-user launch agent under ~/Library/LaunchAgents (not loaded)"
    )]
    #[cfg_attr(
        target_os = "windows",
        doc = "Register manual-start Windows service (does not start)"
    )]
    Install(InstallService),
    #[cfg_attr(
        target_os = "linux",
        doc = "Enroll TPM2-bound unattended credential (seals DEK; prompts for password)"
    )]
    #[cfg_attr(
        target_os = "macos",
        doc = "Enroll Keychain unattended credential (stores DEK; prompts for password)"
    )]
    #[cfg_attr(
        target_os = "windows",
        doc = "Enroll DPAPI machine-scope unattended credential (prompts for password)"
    )]
    EnrollCredential(EnrollCredential),
    /// Internal: run as Windows service (SCM). Not for manual use.
    #[cfg(target_os = "windows")]
    #[command(hide = true)]
    Run(RunService),
    /// Internal (the MSI runs it): restrict the data dir to SYSTEM, Administrators and the
    /// service account. Needs an elevated shell.
    #[cfg(target_os = "windows")]
    #[command(hide = true)]
    RepairAcl(RepairAcl),
}

impl ServiceCmd {
    pub async fn run(self) -> Result<()> {
        match self {
            ServiceCmd::Print(cmd) => cmd.run().await,
            ServiceCmd::Install(cmd) => cmd.run().await,
            ServiceCmd::EnrollCredential(cmd) => cmd.run().await,
            #[cfg(target_os = "windows")]
            ServiceCmd::Run(cmd) => cmd.run().await,
            #[cfg(target_os = "windows")]
            ServiceCmd::RepairAcl(cmd) => cmd.run(),
        }
    }
}

#[derive(ClapArgs, Debug)]
pub struct PrintService {
    #[command(flatten)]
    pub common: Common,
}
impl PrintService {
    pub async fn run(self) -> Result<()> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let paths = ConfigPaths::resolve(self.common.data_dir.as_deref())?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let cfg = config::load_or_init_config(&paths)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let exe = std::env::current_exe().context("resolve current executable")?;
        #[cfg(target_os = "linux")]
        {
            let rendered = crate::service::render_units_with(&cfg, &exe, &paths.data_dir)?;
            println!("{}\n\n{}", rendered.service_unit, rendered.timer_unit);
        }
        #[cfg(target_os = "macos")]
        {
            let rendered = crate::service::render_launchd_with(&cfg, &exe, &paths.data_dir)?;
            println!("{}", rendered.plist);
        }
        #[cfg(target_os = "windows")]
        {
            println!("Windows service uses SCM; no unit file to print.");
            println!("Install via: grindvakt service install\nThen start with: sc start Grindvakt");
        }
        Ok(())
    }
}

#[derive(ClapArgs, Debug)]
pub struct InstallService {
    #[command(flatten)]
    pub common: Common,
}
impl InstallService {
    pub async fn run(self) -> Result<()> {
        let paths = ConfigPaths::resolve(self.common.data_dir.as_deref())?;
        let _cfg = config::load_or_init_config(&paths)?;
        let exe = std::env::current_exe().context("resolve current executable")?;
        InstallUnits::new()
            .install(&_cfg, &exe, &paths.data_dir)
            .await
    }
}

#[derive(ClapArgs, Debug)]
pub struct EnrollCredential {
    #[command(flatten)]
    pub common: Common,
}
impl EnrollCredential {
    pub async fn run(self) -> Result<()> {
        let paths = ConfigPaths::resolve(self.common.data_dir.as_deref())?;
        let cfg = config::load_or_init_config(&paths)?;
        CredentialEnrollment::new(&paths).enroll(&cfg).await
    }
}

#[cfg(target_os = "windows")]
#[derive(ClapArgs, Debug)]
pub struct RepairAcl {
    #[command(flatten)]
    pub common: Common,
}
#[cfg(target_os = "windows")]
impl RepairAcl {
    pub fn run(self) -> Result<()> {
        let paths = ConfigPaths::resolve(self.common.data_dir.as_deref())?;
        crate::winservice::repair_data_dir(&paths)
    }
}

#[cfg(target_os = "windows")]
#[derive(ClapArgs, Debug)]
pub struct RunService {
    #[command(flatten)]
    pub common: Common,
}
#[cfg(target_os = "windows")]
impl RunService {
    pub async fn run(self) -> Result<()> {
        // Data dir may be overridden; pass-through already occurs via launch arguments when installed.
        crate::winservice::run_service()
    }
}
