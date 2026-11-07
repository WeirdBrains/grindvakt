use crate::config::AppConfig;
use crate::dnsclient::{self, DnsAction, DnsimpleClientExt};
use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use secrecy::ExposeSecret;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

pub enum ActiveProvider {
    Dnsimple {
        client: dnsclient::DnsimpleClient,
    },
    Digitalocean {
        client: reqwest::Client,
        api_base: String,
        token: SecretString,
    },
}

pub async fn build_active_provider(
    cfg: &AppConfig,
    token: &SecretString,
) -> Result<ActiveProvider> {
    match cfg.provider {
        crate::config::Provider::Dnsimple => {
            let client = dnsclient::build_client(
                cfg.sandbox,
                token.expose_secret().clone(),
                cfg.api_base_url.clone(),
            )?;
            Ok(ActiveProvider::Dnsimple { client })
        }
        crate::config::Provider::Digitalocean => {
            let client = reqwest::Client::builder().build()?;
            let base = cfg
                .api_base_url
                .clone()
                .unwrap_or_else(|| "https://api.digitalocean.com".to_string());
            Ok(ActiveProvider::Digitalocean {
                client,
                api_base: base,
                token: SecretString::new(token.expose_secret().clone()),
            })
        }
    }
}

impl ActiveProvider {
    pub async fn plan_sync_action(&self, cfg: &AppConfig, ip: Ipv4Addr) -> Result<DnsAction> {
        match self {
            ActiveProvider::Dnsimple { client } => {
                let account_id = dnsclient::resolve_account_id(client, cfg.account_id).await?;
                client.plan_sync_action(cfg, account_id, ip).await
            }
            ActiveProvider::Digitalocean {
                client,
                api_base,
                token,
            } => {
                let name = relative_name_do(cfg);
                let records = list_a_records_do(client, api_base, token, &cfg.zone, &name).await?;
                if records.is_empty() {
                    return Ok(DnsAction::Create { name, ttl: cfg.ttl });
                }
                if records.len() > 1 {
                    return Ok(DnsAction::Ambiguous {
                        reason: format!(
                            "found {} A records for {:?} in {}",
                            records.len(),
                            name,
                            cfg.zone
                        ),
                    });
                }
                let rec = &records[0];
                let current_ip: Ipv4Addr = rec.data.parse().map_err(|_| {
                    anyhow::anyhow!("unexpected non-IPv4 in DO A record: {}", rec.data)
                })?;
                let rec_ttl = rec.ttl.unwrap_or(cfg.ttl as i64) as u32;
                if current_ip == ip && rec_ttl == cfg.ttl {
                    Ok(DnsAction::Noop(current_ip))
                } else {
                    Ok(DnsAction::Update {
                        record_id: rec.id as u64,
                        ttl: cfg.ttl,
                        prev: current_ip,
                    })
                }
            }
        }
    }
    pub async fn create_a_record(
        &self,
        cfg: &AppConfig,
        name: String,
        ip: Ipv4Addr,
        ttl: u32,
    ) -> Result<()> {
        match self {
            ActiveProvider::Dnsimple { client } => {
                let account_id = dnsclient::resolve_account_id(client, cfg.account_id).await?;
                client.create_a_record(account_id, cfg, name, ip, ttl).await
            }
            ActiveProvider::Digitalocean {
                client,
                api_base,
                token,
            } => {
                let url = format!("{}/v2/domains/{}/records", api_base, cfg.zone);
                let body = serde_json::json!({ "type": "A", "name": name, "data": ip.to_string(), "ttl": ttl });
                let resp = client
                    .post(&url)
                    .bearer_auth(token.expose_secret())
                    .json(&body)
                    .send()
                    .await?;
                if !resp.status().is_success() {
                    bail!("digitalocean create unexpected status: {}", resp.status());
                }
                Ok(())
            }
        }
    }
    pub async fn update_a_record(
        &self,
        cfg: &AppConfig,
        record_id: u64,
        ip: Ipv4Addr,
        ttl: u32,
    ) -> Result<()> {
        match self {
            ActiveProvider::Dnsimple { client } => {
                let account_id = dnsclient::resolve_account_id(client, cfg.account_id).await?;
                client
                    .update_a_record(account_id, cfg, record_id, ip, ttl)
                    .await
            }
            ActiveProvider::Digitalocean {
                client,
                api_base,
                token,
            } => {
                let url = format!("{}/v2/domains/{}/records/{}", api_base, cfg.zone, record_id);
                let body = serde_json::json!({ "data": ip.to_string(), "ttl": ttl });
                let resp = client
                    .patch(&url)
                    .bearer_auth(token.expose_secret())
                    .json(&body)
                    .send()
                    .await?;
                if !resp.status().is_success() {
                    bail!("digitalocean update unexpected status: {}", resp.status());
                }
                Ok(())
            }
        }
    }
}

fn relative_name_do(cfg: &AppConfig) -> String {
    let zone = cfg.zone.trim_end_matches('.').to_string();
    let name = cfg.name.trim().trim_end_matches('.');
    if name == "@" || name.is_empty() || name == zone {
        "@".to_string()
    } else if name.ends_with(&zone) {
        let without = name
            .trim_end_matches('.')
            .trim_end_matches(&zone)
            .trim_end_matches('.');
        if without.is_empty() {
            "@".to_string()
        } else {
            without.to_string()
        }
    } else {
        name.to_string()
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct DoListResp {
    domain_records: Vec<DoDomainRecord>,
    #[serde(default)]
    links: Option<DoLinks>,
}
#[derive(Debug, Deserialize, Serialize)]
struct DoLinks {
    #[serde(default)]
    pages: Option<DoPages>,
}
#[derive(Debug, Deserialize, Serialize)]
struct DoPages {
    #[serde(default)]
    next: Option<String>,
}
#[derive(Debug, Deserialize, Serialize, Clone)]
struct DoDomainRecord {
    id: i64,
    #[serde(rename = "type")]
    r#type: String,
    name: String,
    data: String,
    #[serde(default)]
    ttl: Option<i64>,
}

async fn list_a_records_do(
    client: &reqwest::Client,
    base: &str,
    token: &SecretString,
    domain: &str,
    name: &str,
) -> Result<Vec<DoDomainRecord>> {
    let mut page: u32 = 1;
    let mut out = Vec::new();
    let max_pages: u32 = 50;
    loop {
        if page > max_pages {
            bail!("digitalocean pagination exceeded safety cap");
        }
        let url = format!(
            "{}/v2/domains/{}/records?page={}&per_page=200&type=A&name={}",
            base, domain, page, name
        );
        let resp = client
            .get(&url)
            .bearer_auth(token.expose_secret())
            .send()
            .await
            .context("do list request")?;
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || status == StatusCode::TOO_MANY_REQUESTS
        {
            bail!("digitalocean list error: {}", status);
        }
        if !status.is_success() {
            bail!("digitalocean list unexpected status: {}", status);
        }
        let body: DoListResp = resp.json().await.context("parse do list")?;
        out.extend(
            body.domain_records
                .into_iter()
                .filter(|r| r.r#type == "A" && r.name == name),
        );
        let has_next = body
            .links
            .and_then(|l| l.pages)
            .and_then(|p| p.next)
            .is_some();
        if has_next {
            page += 1;
        } else {
            break;
        }
    }
    Ok(out)
}
