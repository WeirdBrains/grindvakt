//! One sync pass: plan the config's `A` record for an address (or its `CNAME` record) and
//! apply it.
use crate::config::AppConfig;
use crate::provider::build_active_provider;
use anyhow::{Result, bail};
use secrecy::SecretString;
use std::net::Ipv4Addr;

pub use grindvakt_core::sync::{CnameOutcome, SyncMode, SyncOutcome};

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

/// Plans and (unless `mode` is a dry run) applies the config's `CNAME` record
/// (`record = "cname"`). Its target comes from the config's `[cname]` table.
pub async fn sync_cname(
    cfg: &AppConfig,
    token: &SecretString,
    mode: SyncMode,
) -> Result<CnameOutcome> {
    let provider = build_active_provider(cfg, token)?;
    match grindvakt_core::sync::sync_cname_once(cfg, &provider, mode).await {
        Ok(outcome) => Ok(outcome),
        Err(grindvakt_core::Error::Ambiguous(reason)) => {
            bail!("Refusing to modify records due to ambiguity: {reason}")
        }
        Err(e) => Err(e.into()),
    }
}
