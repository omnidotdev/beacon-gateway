//! Web fetch tool with SSRF protection
//!
//! Provides HTTP fetching with protection against Server-Side Request Forgery
//! attacks by blocking requests to private/internal IP addresses.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::time::Duration;

use reqwest::{Client, Method};

use crate::{Error, Result};

/// HTTP response from a web fetch
#[derive(Debug, Clone)]
pub struct WebResponse {
    /// HTTP status code
    pub status: u16,
    /// Response headers as key-value pairs
    pub headers: Vec<(String, String)>,
    /// Response body as text
    pub body: String,
    /// Final URL after any redirects
    pub url: String,
}

/// Web fetch tool with SSRF protection
///
/// Provides HTTP fetching capabilities while blocking requests to private
/// and internal IP addresses to prevent SSRF attacks.
pub struct WebFetchTool {
    client: Client,
    #[allow(dead_code)] // Stored for reference; client already configured with timeout
    timeout: Duration,
}

impl WebFetchTool {
    /// Create a new web fetch tool with the specified timeout
    ///
    /// # Errors
    ///
    /// Returns error if the HTTP client cannot be built
    pub fn new(timeout: Duration) -> Result<Self> {
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(10))
            .user_agent("Beacon-Gateway/0.1")
            .build()
            .map_err(Error::Http)?;

        Ok(Self { client, timeout })
    }

    /// Fetch a URL with SSRF protection
    ///
    /// Validates the URL scheme and resolves the hostname to check against
    /// blocked IP ranges before making the request.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - URL scheme is not http or https
    /// - Hostname resolves to a blocked IP address
    /// - HTTP request fails
    /// - Response body cannot be read as text
    pub async fn fetch(&self, url: &str, method: Option<&str>) -> Result<WebResponse> {
        // Parse and validate URL
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| Error::WebFetch(format!("Invalid URL: {e}")))?;

        // Validate scheme
        let scheme = parsed.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(Error::WebFetch(format!(
                "Invalid scheme: {scheme}. Only http and https are allowed"
            )));
        }

        // Get host for SSRF check
        let host = parsed
            .host_str()
            .ok_or_else(|| Error::WebFetch("URL has no host".to_string()))?;

        // Resolve hostname and check for blocked IPs
        Self::check_ssrf(host, parsed.port().unwrap_or(if scheme == "https" { 443 } else { 80 }))?;

        // Parse method
        let method = match method.unwrap_or("GET").to_uppercase().as_str() {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "DELETE" => Method::DELETE,
            "HEAD" => Method::HEAD,
            "OPTIONS" => Method::OPTIONS,
            "PATCH" => Method::PATCH,
            other => {
                return Err(Error::WebFetch(format!("Unsupported HTTP method: {other}")));
            }
        };

        // Make the request
        let response = self
            .client
            .request(method, url)
            .send()
            .await
            .map_err(|e| Error::WebFetch(format!("Request failed: {e}")))?;

        // Extract response details
        let status = response.status().as_u16();
        let final_url = response.url().to_string();

        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    v.to_str().unwrap_or("<binary>").to_string(),
                )
            })
            .collect();

        let body = response
            .text()
            .await
            .map_err(|e| Error::WebFetch(format!("Failed to read response body: {e}")))?;

        Ok(WebResponse {
            status,
            headers,
            body,
            url: final_url,
        })
    }

    /// Check if the host resolves to a blocked IP address
    fn check_ssrf(host: &str, port: u16) -> Result<()> {
        // Try to resolve the hostname
        let addr_str = format!("{host}:{port}");
        let addrs = addr_str
            .to_socket_addrs()
            .map_err(|e| Error::WebFetch(format!("Failed to resolve hostname: {e}")))?;

        // Check all resolved addresses
        for addr in addrs {
            if Self::is_blocked_ip(addr.ip()) {
                return Err(Error::WebFetch(format!(
                    "Blocked: {} resolves to private/internal IP {}",
                    host,
                    addr.ip()
                )));
            }
        }

        Ok(())
    }

    /// Check if an IP address is private or otherwise blocked
    ///
    /// Blocks the following ranges:
    /// - IPv4: 127.0.0.0/8 (loopback), 10.0.0.0/8, 172.16.0.0/12,
    ///   192.168.0.0/16 (private), 169.254.0.0/16 (link-local),
    ///   0.0.0.0/8 (current network)
    /// - IPv6: `::1` (loopback), `fc00::/7` (unique local), `fe80::/10` (link-local)
    #[must_use]
    pub fn is_blocked_ip(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(ipv4) => Self::is_blocked_ipv4(ipv4),
            IpAddr::V6(ipv6) => Self::is_blocked_ipv6(ipv6),
        }
    }

    /// Check if an IPv4 address is blocked
    fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
        let octets = ip.octets();

        // 0.0.0.0/8 - current network
        if octets[0] == 0 {
            return true;
        }

        // 127.0.0.0/8 - loopback
        if octets[0] == 127 {
            return true;
        }

        // 10.0.0.0/8 - private
        if octets[0] == 10 {
            return true;
        }

        // 172.16.0.0/12 - private (172.16.x.x - 172.31.x.x)
        if octets[0] == 172 && (16..=31).contains(&octets[1]) {
            return true;
        }

        // 192.168.0.0/16 - private
        if octets[0] == 192 && octets[1] == 168 {
            return true;
        }

        // 169.254.0.0/16 - link-local
        if octets[0] == 169 && octets[1] == 254 {
            return true;
        }

        false
    }

    /// Check if an IPv6 address is blocked
    fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
        // ::1 - loopback
        if ip.is_loopback() {
            return true;
        }

        let segments = ip.segments();

        // fc00::/7 - unique local addresses (fc00:: - fdff::)
        // Check first byte: 0xfc or 0xfd
        let first_byte = (segments[0] >> 8) as u8;
        if first_byte == 0xfc || first_byte == 0xfd {
            return true;
        }

        // fe80::/10 - link-local addresses
        // First 10 bits must be 1111 1110 10xx xxxx
        if segments[0] & 0xffc0 == 0xfe80 {
            return true;
        }

        // :: - unspecified address
        if ip.is_unspecified() {
            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocked_ipv4_loopback() {
        assert!(WebFetchTool::is_blocked_ip("127.0.0.1".parse().unwrap()));
        assert!(WebFetchTool::is_blocked_ip("127.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_blocked_ipv4_private_10() {
        assert!(WebFetchTool::is_blocked_ip("10.0.0.1".parse().unwrap()));
        assert!(WebFetchTool::is_blocked_ip("10.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_blocked_ipv4_private_172() {
        assert!(WebFetchTool::is_blocked_ip("172.16.0.1".parse().unwrap()));
        assert!(WebFetchTool::is_blocked_ip("172.31.255.255".parse().unwrap()));
        // 172.15.x.x should not be blocked
        assert!(!WebFetchTool::is_blocked_ip("172.15.0.1".parse().unwrap()));
        // 172.32.x.x should not be blocked
        assert!(!WebFetchTool::is_blocked_ip("172.32.0.1".parse().unwrap()));
    }

    #[test]
    fn test_blocked_ipv4_private_192() {
        assert!(WebFetchTool::is_blocked_ip("192.168.0.1".parse().unwrap()));
        assert!(WebFetchTool::is_blocked_ip("192.168.255.255".parse().unwrap()));
        // 192.169.x.x should not be blocked
        assert!(!WebFetchTool::is_blocked_ip("192.169.0.1".parse().unwrap()));
    }

    #[test]
    fn test_blocked_ipv4_link_local() {
        assert!(WebFetchTool::is_blocked_ip("169.254.0.1".parse().unwrap()));
        assert!(WebFetchTool::is_blocked_ip("169.254.255.255".parse().unwrap()));
    }

    #[test]
    fn test_blocked_ipv4_current_network() {
        assert!(WebFetchTool::is_blocked_ip("0.0.0.0".parse().unwrap()));
        assert!(WebFetchTool::is_blocked_ip("0.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_allowed_ipv4() {
        assert!(!WebFetchTool::is_blocked_ip("8.8.8.8".parse().unwrap()));
        assert!(!WebFetchTool::is_blocked_ip("1.1.1.1".parse().unwrap()));
        assert!(!WebFetchTool::is_blocked_ip("93.184.216.34".parse().unwrap()));
    }

    #[test]
    fn test_blocked_ipv6_loopback() {
        assert!(WebFetchTool::is_blocked_ip("::1".parse().unwrap()));
    }

    #[test]
    fn test_blocked_ipv6_unique_local() {
        assert!(WebFetchTool::is_blocked_ip("fc00::1".parse().unwrap()));
        assert!(WebFetchTool::is_blocked_ip("fd00::1".parse().unwrap()));
        assert!(WebFetchTool::is_blocked_ip("fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap()));
    }

    #[test]
    fn test_blocked_ipv6_link_local() {
        assert!(WebFetchTool::is_blocked_ip("fe80::1".parse().unwrap()));
        assert!(WebFetchTool::is_blocked_ip("fe80::1234:5678:abcd:ef01".parse().unwrap()));
    }

    #[test]
    fn test_blocked_ipv6_unspecified() {
        assert!(WebFetchTool::is_blocked_ip("::".parse().unwrap()));
    }

    #[test]
    fn test_allowed_ipv6() {
        assert!(!WebFetchTool::is_blocked_ip("2001:4860:4860::8888".parse().unwrap())); // Google DNS
        assert!(!WebFetchTool::is_blocked_ip("2606:4700:4700::1111".parse().unwrap())); // Cloudflare DNS
    }
}
