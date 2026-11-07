use anyhow::{Result, bail};
use regex::Regex;
use reqwest::Client;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::time::Duration;
use tracing::debug;

#[derive(Clone)]
pub struct PublicIpConsensus {
    pub sources: Vec<&'static str>,
    pub timeout: Duration,
}

impl Default for PublicIpConsensus {
    fn default() -> Self {
        Self {
            sources: vec![
                "https://api.ipify.org",
                "https://ifconfig.me/ip",
                "https://ipv4.icanhazip.com",
                "https://checkip.amazonaws.com",
            ],
            timeout: Duration::from_secs(4),
        }
    }
}

impl PublicIpConsensus {
    pub async fn resolve(&self) -> Result<Ipv4Addr> {
        let client = Client::builder()
            .user_agent("grindvakt/0.1 (+https://weirdbrains.local)")
            .timeout(self.timeout)
            .use_rustls_tls()
            .build()?;
        let re = Regex::new(r"(?m)\b(\d{1,3}(?:\.\d{1,3}){3})\b").unwrap();

        let mut handles = Vec::new();
        for url in &self.sources {
            let c = client.clone();
            let re_c = re.clone();
            let url_s = url.to_string();
            handles.push(tokio::spawn(async move {
                let res = c.get(&url_s).send().await;
                match res {
                    Ok(r) => {
                        let text = r.text().await.unwrap_or_default();
                        let ip = extract_ipv4(&text, &re_c);
                        (url_s, ip)
                    }
                    Err(_) => (url_s, None),
                }
            }));
        }

        let mut seen: HashMap<Ipv4Addr, usize> = HashMap::new();
        let mut success_count = 0usize;
        for h in handles {
            if let Ok((src, ip)) = h.await {
                if let Some(ip) = ip {
                    debug!("source {} -> {}", src, ip);
                    *seen.entry(ip).or_insert(0) += 1;
                    success_count += 1;
                } else {
                    debug!("source {} failed/invalid", src);
                }
            }
        }
        if seen.is_empty() {
            bail!("no public IPv4 sources succeeded");
        }
        let ip = choose_strict_majority(&seen, success_count).ok_or_else(|| {
            anyhow::anyhow!("no consensus on public IPv4 (strict majority required)")
        })?;
        if !is_global_ipv4(ip) {
            bail!("resolved IPv4 not global routable: {}", ip);
        }
        Ok(ip)
    }
}

fn choose_strict_majority(
    counts: &HashMap<Ipv4Addr, usize>,
    success_count: usize,
) -> Option<Ipv4Addr> {
    for (ip, count) in counts {
        if *count >= 2 && *count > success_count / 2 {
            return Some(*ip);
        }
    }
    None
}

fn extract_ipv4(s: &str, re: &Regex) -> Option<Ipv4Addr> {
    if let Some(caps) = re.captures(s) {
        let ip: Ipv4Addr = caps.get(1)?.as_str().parse().ok()?;
        if is_global_ipv4(ip) {
            return Some(ip);
        }
    }
    None
}

pub fn is_global_ipv4(ip: Ipv4Addr) -> bool {
    let oct = ip.octets();
    // RFC1918 private
    if ip.is_private() {
        return false;
    }
    // Loopback
    if ip.is_loopback() {
        return false;
    }
    // Link-local 169.254.0.0/16
    if ip.is_link_local() {
        return false;
    }
    // Multicast
    if ip.is_multicast() {
        return false;
    }
    // Limited broadcast, reserved >=240
    if ip.is_broadcast() || oct[0] >= 240 {
        return false;
    }
    // Documentation 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
    if ip.is_documentation() {
        return false;
    }
    // 0.0.0.0/8
    if oct[0] == 0 {
        return false;
    }
    // CGNAT 100.64.0.0/10
    if oct[0] == 100 && (oct[1] & 0b1100_0000) == 0b0100_0000 {
        return false;
    }
    // 198.18.0.0/15 benchmarking
    if oct[0] == 198 && (oct[1] == 18 || oct[1] == 19) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_normal_global_filtering() {
        assert!(is_global_ipv4("8.8.8.8".parse().unwrap()));
        assert!(!is_global_ipv4("10.0.0.1".parse().unwrap()));
        assert!(!is_global_ipv4("192.168.1.1".parse().unwrap()));
        assert!(!is_global_ipv4("127.0.0.1".parse().unwrap()));
        assert!(!is_global_ipv4("224.0.0.1".parse().unwrap()));
    }

    #[test]
    fn test_majority_selection() {
        let a: Ipv4Addr = "1.1.1.1".parse().unwrap();
        let b: Ipv4Addr = "8.8.8.8".parse().unwrap();
        let mut m = HashMap::new();
        m.insert(a, 2);
        m.insert(b, 1);
        assert_eq!(choose_strict_majority(&m, 3), Some(a));
        let mut m2 = HashMap::new();
        m2.insert(a, 2);
        m2.insert(b, 2);
        assert_eq!(choose_strict_majority(&m2, 4), None);
        let mut m3 = HashMap::new();
        m3.insert(a, 1);
        assert_eq!(choose_strict_majority(&m3, 1), None);
    }
}
