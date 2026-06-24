use std::net::IpAddr;

use ene_tool_common::prelude::*;

const MAX_RESPONSE_SIZE: usize = 5 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;

/// Returns `Ok(())` if `host` resolves (or, when it is already a
/// literal IP, is) an address we are willing to fetch from. The
/// check rejects loopback, private, link-local, and unspecified
/// addresses on both IPv4 and IPv6 to prevent SSRF against
/// localhost, the cloud metadata service, RFC1918 hosts, etc.
///
/// Bare hostnames are resolved via the system resolver before the
/// HTTP client opens a connection, so an attacker cannot bypass
/// the check by pointing the URL at a public DNS name that
/// resolves to a private IP.
async fn assert_safe_host(url: &reqwest::Url) -> Result<(), ToolError> {
    let host = url.host_str().ok_or_else(|| ToolError::InvalidArguments {
        message: "URL must contain a host".to_string(),
    })?;

    let addresses: Vec<IpAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![ip]
    } else {
        // Resolve via the system resolver using the async runtime.
        // The resolver may return multiple addresses (e.g.
        // dual-stack); if ANY of them is unsafe we reject.
        let host_string = host.to_string();
        let addrs = tokio::net::lookup_host(format!("{host_string}:0"))
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("DNS lookup failed for {host}: {e}"),
            })?;
        addrs.map(|s| s.ip()).collect()
    };

    if addresses.is_empty() {
        return Err(ToolError::InvalidArguments {
            message: format!("No addresses resolved for host '{host}'"),
        });
    }

    for ip in &addresses {
        if is_blocked_address(*ip) {
            return Err(ToolError::PermissionRequired {
                request_id: "ssrf-block".to_string(),
                action: "web.fetch".to_string(),
                target: format!("{host} -> {ip}"),
                description: format!(
                    "Refusing to fetch blocked address ({ip}); loopback, \
                     private, link-local, and unspecified ranges are \
                     not allowed"
                ),
            });
        }
    }
    Ok(())
}

fn is_blocked_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()          // 127.0.0.0/8
                || v4.is_private()    // 10/8, 172.16/12, 192.168/16
                || v4.is_link_local()  // 169.254/16 (incl. cloud metadata)
                || v4.is_unspecified() // 0.0.0.0
                || v4.is_broadcast()   // 255.255.255.255
                || v4.is_multicast()   // 224.0.0.0/4
                || v4.is_documentation() // 192.0.2/24, 198.51.100/24, 203.0.113/24
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()    // ::1
                || v6.is_unspecified() // ::
                || v6.is_multicast()
                // Unique-local fc00::/7
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // Link-local fe80::/10
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // IPv4-mapped (::ffff:a.b.c.d) — recurse into v4
                || v6
                    .to_ipv4_mapped()
                    .map(|v4| is_blocked_address(IpAddr::V4(v4)))
                    .unwrap_or(false)
                // IPv4-compatible (deprecated but still routable): ::a.b.c.d
                || v6
                    .to_ipv4()
                    .map(|v4| is_blocked_address(IpAddr::V4(v4)))
                    .unwrap_or(false)
        }
    }
}

fn default_client() -> reqwest::Client {
    reqwest::Client::new()
}

#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "web",
    name = "fetch",
    summary = "Fetch a URL and return its content as text or markdown.",
    description = "Fetches content from a URL and returns it in the requested format (markdown, text, or html). Supports configurable timeout and automatically converts HTML to readable markdown.",
    category = "WebFetch",
    keywords_primary = "fetch, url, web, download, html",
    side_effects = "Network { external: true }"
)]
pub struct WebFetchAction {
    #[tool(skip)]
    #[serde(skip, default = "default_client")]
    client: reqwest::Client,
    /// The URL to fetch content from (must start with http:// or https://).
    url: String,
    /// The format to return: text, markdown, or html. Defaults to markdown.
    #[arg(enum_values = "text, markdown, html", default = "markdown")]
    #[serde(default)]
    format: Option<String>,
    /// Optional timeout in seconds (max 120).
    #[arg(minimum = 1, maximum = 120, default = "30")]
    #[serde(default)]
    timeout: Option<u64>,
}

impl WebFetchAction {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            url: String::new(),
            format: None,
            timeout: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let url = &self.url;
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(ToolError::InvalidArguments {
                message: "URL must start with http:// or https://".to_string(),
            });
        }

        // Parse + SSRF check before doing any network work. A bare
        // hostname is resolved up front so a public DNS name that
        // points at a private IP cannot bypass the check via the
        // OS resolver inside reqwest.
        let parsed = reqwest::Url::parse(url).map_err(|e| ToolError::InvalidArguments {
            message: format!("Invalid URL: {e}"),
        })?;
        assert_safe_host(&parsed).await?;

        let timeout_secs = self
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .min(MAX_TIMEOUT_SECS);
        let format = self.format.as_deref().unwrap_or("markdown");

        let accept_header = match format {
            "text" => "text/plain;q=1.0, text/html;q=0.8, */*;q=0.1",
            "html" => "text/html;q=1.0, text/plain;q=0.8, */*;q=0.1",
            _ => "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        };

        let response = self
            .client
            .get(url)
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .header("Accept", accept_header)
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("HTTP request failed: {e}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "HTTP request returned status: {} {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown")
                ),
            });
        }

        let content_length = response.content_length();
        if let Some(len) = content_length
            && len > MAX_RESPONSE_SIZE as u64
        {
            return Err(ToolError::ExecutionFailed {
                message: "Response too large (exceeds 5MB limit)".to_string(),
            });
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to read response body: {e}"),
            })?;

        if bytes.len() > MAX_RESPONSE_SIZE {
            return Err(ToolError::ExecutionFailed {
                message: "Response too large (exceeds 5MB limit)".to_string(),
            });
        }

        let body = String::from_utf8_lossy(&bytes);

        match format {
            "html" => Ok(body.to_string()),
            _ => Ok(ene_tool_common::html::html_to_markdown(&body)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::net::Ipv6Addr;

    #[test]
    fn blocks_ipv4_loopback() {
        assert!(is_blocked_address(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_blocked_address(IpAddr::V4(Ipv4Addr::new(
            127, 255, 255, 254
        ))));
    }

    #[test]
    fn blocks_ipv4_private() {
        assert!(is_blocked_address(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_blocked_address(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_blocked_address(IpAddr::V4(Ipv4Addr::new(
            192, 168, 1, 1
        ))));
    }

    #[test]
    fn blocks_ipv4_link_local_and_metadata() {
        assert!(is_blocked_address(IpAddr::V4(Ipv4Addr::new(
            169, 254, 169, 254
        ))));
    }

    #[test]
    fn blocks_unspecified_and_broadcast() {
        assert!(is_blocked_address(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));
        assert!(is_blocked_address(IpAddr::V4(Ipv4Addr::new(
            255, 255, 255, 255
        ))));
    }

    #[test]
    fn blocks_ipv6_loopback_and_unspecified() {
        assert!(is_blocked_address(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_blocked_address(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }

    #[test]
    fn blocks_ipv6_ula_and_link_local() {
        // fc00::/7 (unique local)
        assert!(is_blocked_address(IpAddr::V6(Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        ))));
        // fe80::/10 (link local)
        assert!(is_blocked_address(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn blocks_ipv4_mapped_ipv6() {
        // ::ffff:127.0.0.1 — the v4 portion is loopback
        let mapped = Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 0x0001);
        assert!(is_blocked_address(IpAddr::V6(mapped)));
    }

    #[test]
    fn allows_public_ipv4() {
        assert!(!is_blocked_address(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_blocked_address(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }

    #[test]
    fn allows_public_ipv6() {
        // 2606:4700:4700::1111 (Cloudflare DNS, public)
        assert!(!is_blocked_address(IpAddr::V6(Ipv6Addr::new(
            0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111
        ))));
    }

    #[tokio::test]
    async fn rejects_loopback_url() {
        let url = reqwest::Url::parse("http://127.0.0.1/foo").unwrap();
        let err = assert_safe_host(&url).await.unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("Refusing") || msg.contains("Permission"));
    }

    #[tokio::test]
    async fn rejects_metadata_url() {
        let url = reqwest::Url::parse("http://169.254.169.254/latest/meta-data/").unwrap();
        assert!(assert_safe_host(&url).await.is_err());
    }

    #[tokio::test]
    async fn rejects_private_url() {
        let url = reqwest::Url::parse("http://10.0.0.1/secret").unwrap();
        assert!(assert_safe_host(&url).await.is_err());
    }
}
