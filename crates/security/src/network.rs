//! Outbound network policy (SSRF protection).
//!
//! * Only `http`/`https`.
//! * The host must be on the connection's allowlist (empty allowlist = no network access).
//! * Every resolved address is checked. Link-local (incl. cloud metadata 169.254.169.254),
//!   unspecified, multicast and broadcast addresses are always denied. Loopback and private
//!   ranges are denied unless the connection explicitly sets `allow_private_network`.
//! * The HTTP client pins the checked addresses (no DNS rebinding between check and use) and
//!   never follows redirects.

use std::net::{IpAddr, SocketAddr};
use std::sync::Once;
use std::time::Duration;

use ma_types::{MaError, MaResult};
use url::Url;

#[derive(Debug, Clone, Default)]
pub struct NetworkPolicy {
    pub allowed_hosts: Vec<String>,
    pub allow_private_network: bool,
}

impl NetworkPolicy {
    pub fn new(allowed_hosts: Vec<String>, allow_private_network: bool) -> Self {
        NetworkPolicy { allowed_hosts: allowed_hosts.into_iter().map(|h| h.to_ascii_lowercase()).collect(), allow_private_network }
    }

    /// Static checks that need no DNS.
    pub fn check_url_static(&self, url: &Url) -> MaResult<String> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(MaError::policy(format!("scheme `{}` is not allowed", url.scheme())));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(MaError::policy("credentials embedded in URLs are not allowed"));
        }
        let host = url
            .host_str()
            .ok_or_else(|| MaError::policy("URL has no host"))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        if !self.allowed_hosts.iter().any(|h| h == &host) {
            return Err(MaError::policy(format!(
                "host `{host}` is not on this connection's allowlist ({})",
                if self.allowed_hosts.is_empty() { "empty".to_string() } else { self.allowed_hosts.join(", ") }
            )));
        }
        Ok(host)
    }

    pub fn check_ip(&self, ip: IpAddr) -> MaResult<()> {
        let always_denied = match ip {
            IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast() || v4.octets()[0] == 0,
            IpAddr::V6(v6) => v6.is_unspecified() || v6.is_multicast() || (v6.segments()[0] & 0xffc0) == 0xfe80,
        };
        if always_denied {
            return Err(MaError::policy(format!("address {ip} is in a forbidden range (link-local/metadata/unspecified)")));
        }
        let private = match ip {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64),
            IpAddr::V6(v6) => {
                if let Some(m) = v6.to_ipv4_mapped() {
                    return self.check_ip(IpAddr::V4(m));
                }
                v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00
            }
        };
        if private && !self.allow_private_network {
            return Err(MaError::policy(format!(
                "address {ip} is private/loopback; set allow_private_network on the connection to permit it"
            )));
        }
        Ok(())
    }

    /// Full check including DNS resolution. Returns the vetted socket addresses.
    pub async fn check_url(&self, url: &Url) -> MaResult<(String, Vec<SocketAddr>)> {
        let host = self.check_url_static(url)?;
        let port = url.port_or_known_default().unwrap_or(443);
        let addrs: Vec<SocketAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
            vec![SocketAddr::new(ip, port)]
        } else {
            tokio::net::lookup_host((host.as_str(), port))
                .await
                .map_err(|e| MaError::transient(format!("DNS resolution failed for {host}: {e}")))?
                .collect()
        };
        if addrs.is_empty() {
            return Err(MaError::transient(format!("no addresses for {host}")));
        }
        for a in &addrs {
            self.check_ip(a.ip())?;
        }
        Ok((host, addrs))
    }

    /// Build an HTTP client for one base URL with checked, pinned addresses and no redirects.
    pub async fn client_for(&self, base: &Url) -> MaResult<reqwest::Client> {
        install_crypto_provider();
        let (host, addrs) = self.check_url(base).await?;
        let mut b = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15))
            .no_proxy()
            .user_agent(concat!("migrate-anything/", env!("CARGO_PKG_VERSION")));
        if host.parse::<IpAddr>().is_err() {
            b = b.resolve_to_addrs(&host, &addrs);
        }
        b.build().map_err(|e| MaError::internal(format!("http client: {e}")))
    }
}

/// Install the `ring` rustls provider once per process.
pub fn install_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denies_unlisted_and_metadata() {
        let p = NetworkPolicy::new(vec!["api.example.com".into()], false);
        assert!(p.check_url_static(&Url::parse("https://evil.example.com/x").unwrap()).is_err());
        assert!(p.check_url_static(&Url::parse("file:///etc/passwd").unwrap()).is_err());
        assert!(p.check_url_static(&Url::parse("https://u:p@api.example.com/").unwrap()).is_err());
        assert!(p.check_ip("169.254.169.254".parse().unwrap()).is_err());
        assert!(p.check_ip("127.0.0.1".parse().unwrap()).is_err());
        assert!(p.check_ip("10.1.2.3".parse().unwrap()).is_err());
        assert!(p.check_ip("::ffff:127.0.0.1".parse().unwrap()).is_err());
        assert!(p.check_ip("93.184.216.34".parse().unwrap()).is_ok());
        let local = NetworkPolicy::new(vec!["127.0.0.1".into()], true);
        assert!(local.check_ip("127.0.0.1".parse().unwrap()).is_ok());
        assert!(local.check_ip("169.254.169.254".parse().unwrap()).is_err());
    }
}
