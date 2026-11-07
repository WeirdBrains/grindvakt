use crate::config::{self, AppConfig, ConfigPaths, Provider};
use crate::dnsclient::DnsAction;
use crate::ip::PublicIpConsensus;
use crate::service::InstallUnits;
use crate::vault::{
    ChangePasswordFlow, CredentialEnrollment, RecoverFlow, VaultInitFlow,
    set_provider_token_interactive, tokens_status_interactive,
    unlock_token_for_provider_interactive,
};
use anyhow::{Context, Result, bail};
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
    /// Manage systemd --user units and unattended credential
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
        let token = unlock_token_for_provider_interactive(ctx, cfg.provider.clone()).await?;
        let provider = crate::provider::build_active_provider(&cfg, &token).await?;
        // Resolve public IP after verifying credentials to fail early on missing tokens
        let consensus = PublicIpConsensus::default();
        let ip = consensus.resolve().await.context("resolve public IPv4")?;
        let action = provider
            .plan_sync_action(&cfg, ip)
            .await
            .context("plan dns action")?;

        match action {
            DnsAction::Noop(current) => {
                info!("No change needed: {} -> {}", cfg.record_fqdn(), current);
            }
            DnsAction::Create { name, ttl } => {
                if self.dry_run {
                    info!(
                        "DRY RUN: would create A {} = {} (ttl={})",
                        cfg.record_fqdn(),
                        ip,
                        ttl
                    );
                } else {
                    provider
                        .create_a_record(&cfg, name, ip, ttl)
                        .await
                        .context("create a record")?;
                    info!("Created A {} = {} (ttl={})", cfg.record_fqdn(), ip, ttl);
                }
            }
            DnsAction::Update {
                record_id,
                ttl,
                prev,
            } => {
                if self.dry_run {
                    info!(
                        "DRY RUN: would update A {}: {} -> {} (ttl={}) [record_id={}]",
                        cfg.record_fqdn(),
                        prev,
                        ip,
                        ttl,
                        record_id
                    );
                } else {
                    provider
                        .update_a_record(&cfg, record_id, ip, ttl)
                        .await
                        .context("update a record")?;
                    info!(
                        "Updated A {}: {} -> {} (ttl={})",
                        cfg.record_fqdn(),
                        prev,
                        ip,
                        ttl
                    );
                }
            }
            DnsAction::Ambiguous { reason } => {
                bail!("Refusing to modify records due to ambiguity: {reason}");
            }
        }
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
        #[arg(value_enum)]
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
        #[arg(value_enum)]
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
    /// Install service and timer units in ~/.config/systemd/user (not enabled)
    Install(InstallService),
    /// Enroll TPM2-bound unattended credential (seals DEK; prompts for password)
    EnrollCredential(EnrollCredential),
}

impl ServiceCmd {
    pub async fn run(self) -> Result<()> {
        match self {
            ServiceCmd::Print(cmd) => cmd.run().await,
            ServiceCmd::Install(cmd) => cmd.run().await,
            ServiceCmd::EnrollCredential(cmd) => cmd.run().await,
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
        let paths = ConfigPaths::resolve(self.common.data_dir.as_deref())?;
        let cfg = config::load_or_init_config(&paths)?;
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
        let cfg = config::load_or_init_config(&paths)?;
        let exe = std::env::current_exe().context("resolve current executable")?;
        InstallUnits::new()
            .install(&cfg, &exe, &paths.data_dir)
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
// No Windows service subcommands in this focused provider changeset
