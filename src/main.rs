use anyhow::{Context, Result};
use clap::Parser;
use grindvakt as _; // ensure lib crate is linked
use grindvakt::cli;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // Parse first to decide logging (Windows service run initializes its own file logger)
    let args = cli::Args::parse();
    #[cfg(target_os = "windows")]
    let is_service_run = matches!(
        args.command,
        cli::Command::Service {
            cmd: cli::ServiceCmd::Run(_)
        }
    );
    #[cfg(not(target_os = "windows"))]
    let is_service_run = false;
    if !is_service_run {
        let default_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,reqwest=warn"));
        tracing_subscriber::fmt()
            .with_env_filter(default_filter)
            .with_target(false)
            .compact()
            .init();
    }

    match args.command {
        cli::Command::Init(cmd) => cmd.run().await,
        cli::Command::Sync(cmd) => cmd.run().await,
        cli::Command::Provider { cmd } => cmd.run().await,
        cli::Command::Recover(cmd) => cmd.run().await,
        cli::Command::ChangePassword(cmd) => cmd.run().await,
        cli::Command::Service { cmd } => cmd.run().await,
    }
    .context("grindvakt failed")
}
