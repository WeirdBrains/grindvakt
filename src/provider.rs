//! DNS providers, from `grindvakt_core::provider`.
use crate::config::AppConfig;
use anyhow::Result;
use secrecy::SecretString;

pub use grindvakt_core::provider::{AnyProvider, DnsProvider};

/// Builds the provider named in `cfg` (DNSimple or DigitalOcean) with `token`.
pub fn build_active_provider(cfg: &AppConfig, token: &SecretString) -> Result<AnyProvider> {
    Ok(grindvakt_core::provider::build(cfg, token)?)
}
