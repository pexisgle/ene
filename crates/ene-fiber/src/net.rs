//! Host-side HTTP fetch for [`crate::Broker::net_fetch`].

use crate::broker::BrokerError;
use serde_json::{Value, json};
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;
use url::Url;

pub(crate) const MAX_BODY_BYTES: usize = 1_048_576;

#[cfg(test)]
type FetchStub = fn(&str) -> Result<Value, BrokerError>;

#[cfg(test)]
thread_local! {
    static FETCH_STUB: std::cell::RefCell<Option<FetchStub>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_fetch_stub<T>(stub: FetchStub, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FETCH_STUB.with(|cell| cell.replace(None));
        }
    }
    FETCH_STUB.with(|cell| cell.replace(Some(stub)));
    let _reset = Reset;
    run()
}

pub(crate) fn get(raw: &str) -> Result<Value, BrokerError> {
    let url = deny_ssrf(raw)?;
    #[cfg(test)]
    if let Some(stub) = FETCH_STUB.with(|cell| *cell.borrow()) {
        return stub(url.as_str());
    }
    fetch(&url)
}

pub(crate) fn deny_ssrf(raw: &str) -> Result<Url, BrokerError> {
    let url = Url::parse(raw).map_err(|err| BrokerError::InvalidUrl(err.to_string()))?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err(BrokerError::Ssrf(
            "only http(s) URLs are allowed".to_owned(),
        ));
    }
    match url.host() {
        Some(url::Host::Domain(host)) => {
            if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
                return Err(BrokerError::Ssrf("private hosts are blocked".to_owned()));
            }
        }
        Some(url::Host::Ipv4(v4)) => {
            if is_private(IpAddr::V4(v4)) {
                return Err(BrokerError::Ssrf("private hosts are blocked".to_owned()));
            }
        }
        Some(url::Host::Ipv6(v6)) => {
            if is_private(IpAddr::V6(v6)) {
                return Err(BrokerError::Ssrf("private hosts are blocked".to_owned()));
            }
        }
        None => return Err(BrokerError::Ssrf("URL has no host".to_owned())),
    }
    Ok(url)
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private(IpAddr::V4(v4));
            }
            v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local()
        }
    }
}

fn fetch(url: &Url) -> Result<Value, BrokerError> {
    let url = url.clone();
    off_runtime(move || {
        let client = pinned_client(&url)?;
        let response = client
            .get(url.clone())
            .send()
            .map_err(|err| BrokerError::Fetch(err.to_string()))?;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let limit = u64::try_from(MAX_BODY_BYTES.saturating_add(1)).unwrap_or(u64::MAX);
        let mut body = Vec::new();
        response
            .take(limit)
            .read_to_end(&mut body)
            .map_err(|err| BrokerError::Fetch(err.to_string()))?;
        if body.len() > MAX_BODY_BYTES {
            return Err(BrokerError::Fetch("response exceeds limit".to_owned()));
        }
        let text = String::from_utf8_lossy(&body).into_owned();
        Ok(json!({
            "status": status,
            "content_type": content_type,
            "text": text,
        }))
    })
}

fn pinned_client(url: &Url) -> Result<reqwest::blocking::Client, BrokerError> {
    drop(rustls::crypto::ring::default_provider().install_default());
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none());
    if let Some(addr) = resolve_public(url)? {
        let host = url
            .host_str()
            .ok_or_else(|| BrokerError::Ssrf("URL has no host".to_owned()))?;
        builder = builder.resolve(host, addr);
    }
    builder
        .build()
        .map_err(|err| BrokerError::Fetch(err.to_string()))
}

fn resolve_public(url: &Url) -> Result<Option<SocketAddr>, BrokerError> {
    match url.host() {
        Some(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => Ok(None),
        Some(url::Host::Domain(host)) => {
            let port = url.port_or_known_default().unwrap_or(80);
            let mut chosen = None;
            let mut resolved = false;
            for addr in (host, port)
                .to_socket_addrs()
                .map_err(|err| BrokerError::Fetch(err.to_string()))?
            {
                resolved = true;
                if is_private(addr.ip()) {
                    return Err(BrokerError::Ssrf("private hosts are blocked".to_owned()));
                }
                chosen.get_or_insert(addr);
            }
            if !resolved {
                return Err(BrokerError::Ssrf("host did not resolve".to_owned()));
            }
            Ok(chosen)
        }
        None => Err(BrokerError::Ssrf("URL has no host".to_owned())),
    }
}

fn off_runtime<T: Send>(
    work: impl FnOnce() -> Result<T, BrokerError> + Send,
) -> Result<T, BrokerError> {
    std::thread::scope(|scope| {
        scope
            .spawn(work)
            .join()
            .unwrap_or_else(|_| Err(BrokerError::Fetch("fetch worker panicked".to_owned())))
    })
}

#[cfg(test)]
mod tests {
    use super::deny_ssrf;
    use crate::broker::BrokerError;

    #[test]
    fn deny_ssrf_blocks_loopback_and_private() {
        for raw in [
            "http://127.0.0.1/secret",
            "http://localhost/secret",
            "http://foo.localhost/",
            "http://10.0.0.1/",
            "http://192.168.1.1/",
            "http://[::1]/",
            "http://[::ffff:127.0.0.1]/",
            "file:///etc/passwd",
        ] {
            assert!(
                matches!(
                    deny_ssrf(raw),
                    Err(BrokerError::Ssrf(_) | BrokerError::InvalidUrl(_))
                ),
                "{raw}"
            );
        }
    }

    #[test]
    fn deny_ssrf_allows_public_https() {
        let url = deny_ssrf("https://example.invalid/v1").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("example.invalid"));
    }

    #[test]
    fn resolve_public_blocks_localhost() {
        let url = url::Url::parse("http://localhost/").unwrap();
        assert!(matches!(
            super::resolve_public(&url),
            Err(BrokerError::Ssrf(_))
        ));
    }
}
