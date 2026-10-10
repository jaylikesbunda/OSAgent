//! Network egress policy for the browser sandbox.
//!
//! Applied to every request the page makes (via CDP `Fetch` interception)
//! and to navigation targets before they are sent. Mirrors the guarantee
//! `public_web_fetch` gives: by default nothing on loopback, private or
//! link-local networks is reachable from a page the agent opened.

use crate::config::BrowserConfig;
use crate::tools::web::PublicWebFetchTool;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DNS_CACHE_TTL: Duration = Duration::from_secs(60);

pub struct EgressPolicy {
    allowed_hosts: Vec<String>,
    blocked_hosts: Vec<String>,
    block_private_network: bool,
    dns_cache: Mutex<HashMap<String, (Instant, bool)>>,
}

impl EgressPolicy {
    pub fn from_config(config: &BrowserConfig) -> Self {
        Self {
            allowed_hosts: normalize(&config.allowed_hosts),
            blocked_hosts: normalize(&config.blocked_hosts),
            block_private_network: config.block_private_network,
            dns_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Whether any request needs inspecting. When false the browser skips
    /// installing the interception hook entirely.
    pub fn is_active(&self) -> bool {
        self.block_private_network
            || !self.allowed_hosts.is_empty()
            || !self.blocked_hosts.is_empty()
    }

    /// `Ok(())` if the URL may be requested; otherwise a human-readable reason.
    pub async fn check(&self, raw_url: &str) -> Result<(), String> {
        let url = reqwest::Url::parse(raw_url).map_err(|_| format!("invalid URL: {raw_url}"))?;
        match url.scheme() {
            "http" | "https" => {}
            // Inline content the page already holds; no network involved.
            "data" | "blob" | "about" => return Ok(()),
            // Chromium's own bundled component resources (extensions are
            // otherwise disabled); they never touch the network.
            "chrome-extension" => return Ok(()),
            other => return Err(format!("`{other}:` URLs are not allowed in the browser")),
        }
        let host = url
            .host_str()
            .ok_or_else(|| "URL has no host".to_string())?
            .trim_matches(|c| c == '[' || c == ']')
            .to_ascii_lowercase();

        if self.blocked_hosts.iter().any(|p| host_matches(p, &host)) {
            return Err(format!("host {host} is blocked by [browser] blocked_hosts"));
        }
        if !self.allowed_hosts.is_empty() && !self.allowed_hosts.iter().any(|p| host_matches(p, &host))
        {
            return Err(format!("host {host} is not in [browser] allowed_hosts"));
        }
        if self.block_private_network && !self.host_is_public(&host).await {
            return Err(format!(
                "host {host} resolves to a local or private network address"
            ));
        }
        Ok(())
    }

    async fn host_is_public(&self, host: &str) -> bool {
        if host == "localhost" || host.ends_with(".localhost") {
            return false;
        }
        if let Ok(ip) = host.parse::<IpAddr>() {
            return PublicWebFetchTool::is_public_ip(ip);
        }
        if let Some((at, public)) = self.dns_cache.lock().expect("dns cache lock").get(host) {
            if at.elapsed() < DNS_CACHE_TTL {
                return *public;
            }
        }
        let public = match tokio::net::lookup_host((host, 443)).await {
            Ok(addresses) => {
                let addresses: Vec<_> = addresses.collect();
                !addresses.is_empty()
                    && addresses
                        .iter()
                        .all(|address| PublicWebFetchTool::is_public_ip(address.ip()))
            }
            // Unresolvable hosts cannot reach anything; let the browser
            // surface its own DNS error.
            Err(_) => true,
        };
        let mut cache = self.dns_cache.lock().expect("dns cache lock");
        if cache.len() > 512 {
            cache.clear();
        }
        cache.insert(host.to_string(), (Instant::now(), public));
        public
    }
}

fn normalize(patterns: &[String]) -> Vec<String> {
    patterns
        .iter()
        .map(|pattern| pattern.trim().to_ascii_lowercase())
        .filter(|pattern| !pattern.is_empty())
        .collect()
}

/// `example.com` matches that host exactly; `*.example.com` matches the
/// apex and any subdomain.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
        None => host == pattern,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(allowed: &[&str], blocked: &[&str], private: bool) -> EgressPolicy {
        EgressPolicy::from_config(&BrowserConfig {
            allowed_hosts: allowed.iter().map(|s| s.to_string()).collect(),
            blocked_hosts: blocked.iter().map(|s| s.to_string()).collect(),
            block_private_network: private,
            ..BrowserConfig::default()
        })
    }

    #[test]
    fn wildcard_matches_apex_and_subdomains_only() {
        assert!(host_matches("*.docs.rs", "docs.rs"));
        assert!(host_matches("*.docs.rs", "api.docs.rs"));
        assert!(!host_matches("*.docs.rs", "evildocs.rs"));
        assert!(host_matches("example.com", "example.com"));
        assert!(!host_matches("example.com", "www.example.com"));
    }

    #[tokio::test]
    async fn blocks_private_and_loopback_literals() {
        let policy = policy(&[], &[], true);
        for url in [
            "http://127.0.0.1:8080/",
            "http://localhost/admin",
            "http://192.168.1.10/",
            "http://10.0.0.5/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
        ] {
            assert!(policy.check(url).await.is_err(), "{url} should be blocked");
        }
        assert!(policy.check("https://1.1.1.1/").await.is_ok());
    }

    #[tokio::test]
    async fn private_blocking_can_be_disabled() {
        let policy = policy(&[], &[], false);
        assert!(policy.check("http://127.0.0.1:3000/").await.is_ok());
    }

    #[tokio::test]
    async fn rejects_dangerous_schemes_but_allows_inline() {
        let policy = policy(&[], &[], true);
        assert!(policy.check("file:///etc/passwd").await.is_err());
        assert!(policy.check("javascript:alert(1)").await.is_err());
        assert!(policy.check("chrome://settings").await.is_err());
        assert!(policy.check("data:text/plain,hi").await.is_ok());
        assert!(policy.check("about:blank").await.is_ok());
    }

    #[tokio::test]
    async fn allow_and_block_lists_apply() {
        let policy = policy(&["*.example.com"], &["bad.example.com"], false);
        assert!(policy.check("https://www.example.com/").await.is_ok());
        assert!(policy.check("https://bad.example.com/").await.is_err());
        assert!(policy.check("https://other.org/").await.is_err());
    }

    #[test]
    fn inactive_when_nothing_to_enforce() {
        assert!(!policy(&[], &[], false).is_active());
        assert!(policy(&[], &[], true).is_active());
    }
}
