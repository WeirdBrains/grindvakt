//! Public IPv4 discovery through `grindvakt_core::ip` (majority of HTTPS echo services,
//! IPv4-only connections, non-global answers rejected).

use anyhow::{Context, Result};
use grindvakt_core::ip::{Family, Sources, discover};
use std::net::Ipv4Addr;

pub use grindvakt_core::ip::{is_global, is_global_ipv4, is_global_ipv6};

/// Resolves the public IPv4 address from a majority of independent sources.
#[derive(Debug, Clone, Default)]
pub struct PublicIpConsensus {
    /// Where to ask; defaults to `grindvakt_core::ip::DEFAULT_V4_SOURCES`.
    pub sources: Sources,
}

impl PublicIpConsensus {
    /// The agreed public IPv4 address, or an error when there is none.
    pub async fn resolve(&self) -> Result<Ipv4Addr> {
        let found = discover(Family::V4, &self.sources).await?;
        found.v4.context("no public IPv4 address")
    }
}
