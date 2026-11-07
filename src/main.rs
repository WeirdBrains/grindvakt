use anyhow::{Context, Result};
use clap::Parser;
use grindvakt as _; // ensure lib crate is linked
use grindvakt::cli;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // Initialize logging early; default INFO with override via RUST_LOG.
    let default_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,hyper=warn,reqwest=warn"));
    tracing_subscriber::fmt()
        .with_env_filter(default_filter)
        .with_target(false)
        .compact()
        .init();

    let args = cli::Args::parse();

    match args.command {
        cli::Command::Init(cmd) => cmd.run().await,
        cli::Command::Sync(cmd) => cmd.run().await,
        cli::Command::Recover(cmd) => cmd.run().await,
        cli::Command::ChangePassword(cmd) => cmd.run().await,
        cli::Command::Service { cmd } => cmd.run().await,
    }
    .context("grindvakt failed")
}
