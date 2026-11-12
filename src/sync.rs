//! One sync pass: plan the config's `A` record for an address and apply it.
use crate::config::AppConfig;
use crate::provider::build_active_provider;
use anyhow::{Result, bail};
use secrecy::SecretString;
use std::net::Ipv4Addr;

pub use grindvakt_core::sync::{SyncMode, SyncOutcome};

/// Plans and (unless `mode` is a dry run) applies the record for `ip`.
pub async fn sync_record(
    cfg: &AppConfig,
    token: &SecretString,
    ip: Ipv4Addr,
    mode: SyncMode,
) -> Result<SyncOutcome> {
    let provider = build_active_provider(cfg, token)?;
    match grindvakt_core::sync::sync_once(cfg, &provider, ip, mode).await {
        Ok(outcome) => Ok(outcome),
        Err(grindvakt_core::Error::Ambiguous(reason)) => {
            bail!("Refusing to modify records due to ambiguity: {reason}")
        }
        Err(e) => Err(e.into()),
    }
}
