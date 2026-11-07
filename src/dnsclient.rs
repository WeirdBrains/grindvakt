use crate::config::AppConfig;
use ::dnsimple::dnsimple::zones_records::{ZoneRecord, ZoneRecordPayload, ZoneRecordUpdatePayload};
use ::dnsimple::dnsimple::{Client, Filters, Paginate, RequestOptions, new_client};
use ::dnsimple::errors::DNSimpleError;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::Duration;

pub type DnsimpleClient = Client;

pub fn build_client(
    sandbox: bool,
    token: String,
    base_url: Option<String>,
) -> Result<DnsimpleClient> {
    let mut client = new_client(sandbox, token).context("create dnsimple client")?;
    if let Some(base) = base_url {
        client.set_base_url(&base);
    }
    Ok(client)
}

pub async fn resolve_account_id(
    client: &DnsimpleClient,
    account_id_override: Option<u64>,
) -> Result<u64> {
    if let Some(id) = account_id_override {
        return Ok(id);
    }
    let resp = client.identity().whoami().await.context("whoami")?;
    let data = resp.data.context("whoami returned no data")?;
    if let Some(acct) = data.account {
        Ok(acct.id)
    } else {
        bail!("whoami did not include account id; provide account_id in config");
    }
}

#[derive(Debug, Clone)]
pub enum DnsAction {
    Noop(Ipv4Addr),
    Create {
        name: String,
        ttl: u32,
    },
    Update {
        record_id: u64,
        ttl: u32,
        prev: Ipv4Addr,
    },
    Ambiguous {
        reason: String,
    },
}

#[allow(async_fn_in_trait)]
pub trait DnsimpleClientExt {
    fn relative_name(&self, cfg: &AppConfig) -> String;
    async fn plan_sync_action(
        &self,
        cfg: &AppConfig,
        account_id: u64,
        ip: Ipv4Addr,
    ) -> Result<DnsAction>;
    async fn create_a_record(
        &self,
        account_id: u64,
        cfg: &AppConfig,
        name: String,
        ip: Ipv4Addr,
        ttl: u32,
    ) -> Result<()>;
    async fn update_a_record(
        &self,
        account_id: u64,
        cfg: &AppConfig,
        record_id: u64,
        ip: Ipv4Addr,
        ttl: u32,
    ) -> Result<()>;
}

impl DnsimpleClientExt for DnsimpleClient {
    fn relative_name(&self, cfg: &AppConfig) -> String {
        let zone = cfg.zone.trim_end_matches('.').to_string();
        let name = cfg.name.trim().trim_end_matches('.');
        if name == "@" || name.is_empty() || name == zone {
            "".to_string()
        } else if name.ends_with(&zone) {
            // FQDN: strip the suffix
            let without = name
                .trim_end_matches('.')
                .trim_end_matches(&zone)
                .trim_end_matches('.');
            without.to_string()
        } else {
            name.to_string()
        }
    }

    async fn plan_sync_action(
        &self,
        cfg: &AppConfig,
        account_id: u64,
        ip: Ipv4Addr,
    ) -> Result<DnsAction> {
        let name = self.relative_name(cfg);
        let mut matches = list_a_records_by_name(self, account_id, &cfg.zone, &name).await?;
        if matches.is_empty() {
            return Ok(DnsAction::Create { name, ttl: cfg.ttl });
        }
        // Narrow to exact name-type matches only (should already be)
        // If more than one A record exists for that name, we refuse to guess
        if matches.len() > 1 {
            return Ok(DnsAction::Ambiguous {
                reason: format!(
                    "found {} A records for name {:?} in zone {}; specify record_id manually or clean up duplicates",
                    matches.len(),
                    name,
                    cfg.zone
                ),
            });
        }
        let rec = matches.remove(0);
        let current_ip: Ipv4Addr = rec.content.parse().map_err(|_| {
            anyhow::anyhow!(
                "unexpected non-IPv4 content in existing A record: {}",
                rec.content
            )
        })?;
        if current_ip == ip && rec.ttl == cfg.ttl as u64 {
            return Ok(DnsAction::Noop(current_ip));
        }
        Ok(DnsAction::Update {
            record_id: rec.id,
            ttl: cfg.ttl,
            prev: current_ip,
        })
    }

    async fn create_a_record(
        &self,
        account_id: u64,
        cfg: &AppConfig,
        name: String,
        ip: Ipv4Addr,
        ttl: u32,
    ) -> Result<()> {
        // To avoid duplicate POST on retries, always recheck before creating.
        let existing = list_a_records_by_name(self, account_id, &cfg.zone, &name).await?;
        if !existing.is_empty() {
            bail!("refusing to create: record now exists; re-run sync");
        }
        let payload = ZoneRecordPayload {
            name,
            record_type: "A".to_string(),
            content: ip.to_string(),
            ttl: Some(ttl as u64),
            priority: None,
            regions: None,
        };
        let _ = self
            .zones()
            .create_zone_record(account_id, &cfg.zone, payload)
            .await
            .with_context(|| format!("create_zone_record {}/{}", cfg.zone, cfg.name))?;
        Ok(())
    }

    async fn update_a_record(
        &self,
        account_id: u64,
        cfg: &AppConfig,
        record_id: u64,
        ip: Ipv4Addr,
        ttl: u32,
    ) -> Result<()> {
        let payload = ZoneRecordUpdatePayload {
            name: None,
            content: Some(ip.to_string()),
            ttl: Some(ttl as u64),
            priority: None,
            regions: None,
        };
        let _ = self
            .zones()
            .update_zone_record(account_id, &cfg.zone, record_id, payload)
            .await
            .with_context(|| format!("update_zone_record id={}", record_id))?;
        Ok(())
    }
}

async fn list_a_records_by_name(
    client: &DnsimpleClient,
    account_id: u64,
    zone: &str,
    name: &str,
) -> Result<Vec<ZoneRecord>> {
    let mut page: u32 = 1;
    let mut out = Vec::new();
    loop {
        let resp = with_retries(|| async {
            let mut fmap = HashMap::new();
            fmap.insert("type".to_string(), "A".to_string());
            if !name.is_empty() {
                fmap.insert("name".to_string(), name.to_string());
            }
            let filters = Filters::new(fmap);
            let paginate = Paginate {
                per_page: 100,
                page,
            };
            let opts = RequestOptions {
                filters: Some(filters),
                sort: None,
                paginate: Some(paginate),
            };
            client
                .zones()
                .list_zone_records(account_id, zone, Some(opts))
                .await
        })
        .await
        .with_context(|| format!("list_zone_records page {}", page))?;
        if let Some(records) = resp.data {
            for r in records {
                if r.record_type == "A" && record_name_matches(&r, name) {
                    out.push(r);
                }
            }
        }
        let pagination = resp.pagination.context("missing pagination")?;
        if u64::from(page) >= pagination.total_pages {
            break;
        }
        page += 1;
    }
    Ok(out)
}

fn record_name_matches(r: &ZoneRecord, want_name: &str) -> bool {
    let rname = r.name.clone();
    if want_name.is_empty() {
        rname.is_empty()
    } else {
        rname == want_name
    }
}

async fn with_retries<F, Fut, T>(mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, DNSimpleError>>,
{
    let mut attempt = 0;
    let max_attempts = 3;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                // Fail fast on auth and rate limiting; retry only on selected transients.
                let is_auth_or_rate = matches!(
                    e,
                    DNSimpleError::Unauthorized
                        | DNSimpleError::TooManyRequests
                        | DNSimpleError::Forbidden(_)
                );
                if is_auth_or_rate {
                    return Err(anyhow::anyhow!(e));
                }
                let should_retry = matches!(
                    &e,
                    DNSimpleError::BadGateway
                        | DNSimpleError::GatewayTimeout(_)
                        | DNSimpleError::ServiceUnavailable
                        | DNSimpleError::Network(_)
                ) || matches!(&e, DNSimpleError::UnexpectedStatus(c) if (500..=599).contains(c));
                if !should_retry {
                    return Err(anyhow::anyhow!(e));
                }
                attempt += 1;
                if attempt >= max_attempts {
                    return Err(anyhow::anyhow!(e));
                }
                tokio::time::sleep(Duration::from_millis(200u64 * attempt as u64)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_relative_name() {
        let cfg = AppConfig {
            account_id: None,
            zone: "example.com".into(),
            name: "@".into(),
            ttl: 300,
            sandbox: false,
            api_base_url: None,
            schedule: Default::default(),
        };
        let client = ::dnsimple::dnsimple::new_client(false, "t".into()).unwrap();
        assert_eq!(client.relative_name(&cfg), "");
        let mut cfg2 = cfg.clone();
        cfg2.name = "www".into();
        assert_eq!(client.relative_name(&cfg2), "www");
        let mut cfg3 = cfg.clone();
        cfg3.name = "api.example.com".into();
        assert_eq!(client.relative_name(&cfg3), "api");
    }
}
