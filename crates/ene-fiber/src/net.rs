//! Host-side HTTP fetch for [`crate::Broker::net_fetch`].
//!
//! Redirects are followed only after each hop passes URL, DNS, and IP checks.
//! DNS answers are pinned onto the client so a later rebinding cannot retarget
//! the connection. Response bodies are streamed to a byte cap.

use crate::broker::BrokerError;
use serde_json::{Value, json};
use std::io::Read;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::time::Duration;
use url::Url;

pub(crate) const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_REDIRECTS: usize = 5;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const USER_AGENT: &str = "ene-web/0.1";

#[cfg(test)]
type FetchStub = fn(&str) -> Result<Value, BrokerError>;

#[cfg(test)]
type PostStub = fn(&str, &Value, Option<&str>) -> Value;

#[cfg(test)]
type HopStub = fn(&str, Option<&str>) -> Result<HopResponse, BrokerError>;

#[cfg(test)]
type DnsStub = fn(&str) -> Vec<IpAddr>;

#[cfg(test)]
thread_local! {
    static FETCH_STUB: std::cell::RefCell<Option<FetchStub>> = const { std::cell::RefCell::new(None) };
    static POST_STUB: std::cell::RefCell<Option<PostStub>> = const { std::cell::RefCell::new(None) };
    static HOP_STUB: std::cell::RefCell<Option<HopStub>> = const { std::cell::RefCell::new(None) };
    static DNS_STUB: std::cell::RefCell<Option<DnsStub>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_fetch_stub<T>(stub: FetchStub, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FETCH_STUB.with(|cell| {
                cell.replace(None);
            });
        }
    }
    FETCH_STUB.with(|cell| cell.replace(Some(stub)));
    let _reset = Reset;
    run()
}

#[cfg(test)]
pub(crate) fn with_post_stub<T>(stub: PostStub, run: impl FnOnce() -> T) -> T {
    POST_STUB.with(|cell| cell.replace(Some(stub)));
    run()
}

#[cfg(test)]
fn with_hop_stub<T>(stub: HopStub, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            HOP_STUB.with(|cell| {
                cell.replace(None);
            });
        }
    }
    HOP_STUB.with(|cell| cell.replace(Some(stub)));
    let _reset = Reset;
    run()
}

#[cfg(test)]
fn with_dns_stub<T>(stub: DnsStub, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            DNS_STUB.with(|cell| {
                cell.replace(None);
            });
        }
    }
    DNS_STUB.with(|cell| cell.replace(Some(stub)));
    let _reset = Reset;
    run()
}

pub(crate) fn get(raw: &str) -> Result<Value, BrokerError> {
    get_inject(raw, None)
}

pub(crate) fn get_inject(raw: &str, authorization: Option<&str>) -> Result<Value, BrokerError> {
    let url = deny_ssrf(raw)?;
    #[cfg(test)]
    if let Some(stub) = FETCH_STUB.with(|cell| *cell.borrow()) {
        return stub(url.as_str());
    }
    let authorization = authorization.map(str::to_owned);
    #[cfg(test)]
    if HOP_STUB.with(|cell| cell.borrow().is_some()) {
        return fetch_follow(&url, authorization.as_deref());
    }
    off_runtime(move || fetch_follow(&url, authorization.as_deref()))
}

/// POST JSON with the same SSRF, redirect, and body-cap discipline as GET.
/// `bearer` is sent only on same-origin hops.
pub(crate) fn post_json(
    raw: &str,
    body: &Value,
    bearer: Option<&str>,
) -> Result<Value, BrokerError> {
    let url = deny_ssrf(raw)?;
    #[cfg(test)]
    if let Some(stub) = POST_STUB.with(|cell| *cell.borrow()) {
        return Ok(stub(url.as_str(), body, bearer));
    }
    #[cfg(test)]
    if let Some(stub) = FETCH_STUB.with(|cell| *cell.borrow()) {
        let _ = body;
        return stub(url.as_str());
    }
    let bearer = bearer.map(str::to_owned);
    #[cfg(test)]
    if HOP_STUB.with(|cell| cell.borrow().is_some()) {
        return post_follow(&url, body, bearer.as_deref());
    }
    off_runtime(move || post_follow(&url, body, bearer.as_deref()))
}

fn post_follow(start: &Url, body: &Value, bearer: Option<&str>) -> Result<Value, BrokerError> {
    let mut current = start.clone();
    let mut hops = 0_usize;
    loop {
        deny_ssrf(current.as_str())?;
        let hop_auth = same_origin(start, &current).then_some(bearer).flatten();
        let hop = perform_post(&current, body, hop_auth)?;
        if (300..400).contains(&hop.status) {
            hops = hops.saturating_add(1);
            if hops > MAX_REDIRECTS {
                return Err(BrokerError::RedirectLoop);
            }
            let location = hop
                .location
                .ok_or_else(|| BrokerError::Fetch("redirect missing location".to_owned()))?;
            current = current
                .join(&location)
                .map_err(|err| BrokerError::InvalidUrl(err.to_string()))?;
            continue;
        }
        return finalize_body(hop);
    }
}

fn perform_post(
    url: &Url,
    body: &Value,
    authorization: Option<&str>,
) -> Result<HopResponse, BrokerError> {
    #[cfg(test)]
    if let Some(stub) = HOP_STUB.with(|cell| *cell.borrow()) {
        return stub(url.as_str(), authorization);
    }
    let client = pinned_client(url)?;
    let mut request = client
        .post(url.clone())
        .json(body)
        .header(reqwest::header::USER_AGENT, USER_AGENT);
    if let Some(value) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }
    let response = request.send().map_err(|err| classify_reqwest(&err))?;
    let status = response.status().as_u16();
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if (300..400).contains(&status) {
        return Ok(HopResponse {
            status,
            location,
            content_type,
            body: Vec::new(),
        });
    }
    let limit = u64::try_from(MAX_BODY_BYTES.saturating_add(1)).unwrap_or(u64::MAX);
    let mut payload = Vec::new();
    response
        .take(limit)
        .read_to_end(&mut payload)
        .map_err(|err| BrokerError::Fetch(err.to_string()))?;
    if payload.len() > MAX_BODY_BYTES {
        return Err(BrokerError::Oversize);
    }
    Ok(HopResponse {
        status,
        location,
        content_type,
        body: payload,
    })
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

struct HopResponse {
    status: u16,
    location: Option<String>,
    content_type: String,
    body: Vec<u8>,
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn fetch_follow(start: &Url, authorization: Option<&str>) -> Result<Value, BrokerError> {
    let mut current = start.clone();
    let mut hops = 0_usize;
    loop {
        deny_ssrf(current.as_str())?;
        let hop_auth = same_origin(start, &current)
            .then_some(authorization)
            .flatten();
        let hop = perform_hop(&current, hop_auth)?;
        if (300..400).contains(&hop.status) {
            hops = hops.saturating_add(1);
            if hops > MAX_REDIRECTS {
                return Err(BrokerError::RedirectLoop);
            }
            let location = hop
                .location
                .ok_or_else(|| BrokerError::Fetch("redirect missing location".to_owned()))?;
            current = current
                .join(&location)
                .map_err(|err| BrokerError::InvalidUrl(err.to_string()))?;
            continue;
        }
        return finalize_body(hop);
    }
}

fn perform_hop(url: &Url, authorization: Option<&str>) -> Result<HopResponse, BrokerError> {
    #[cfg(test)]
    if let Some(stub) = HOP_STUB.with(|cell| *cell.borrow()) {
        return stub(url.as_str(), authorization);
    }
    let client = pinned_client(url)?;
    let mut request = client
        .get(url.clone())
        .header(reqwest::header::USER_AGENT, USER_AGENT);
    if let Some(value) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }
    let response = request.send().map_err(|err| classify_reqwest(&err))?;
    let status = response.status().as_u16();
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if (300..400).contains(&status) {
        return Ok(HopResponse {
            status,
            location,
            content_type,
            body: Vec::new(),
        });
    }
    if !is_text_content_type(&content_type) {
        return Err(BrokerError::Binary);
    }
    let limit = u64::try_from(MAX_BODY_BYTES.saturating_add(1)).unwrap_or(u64::MAX);
    let mut body = Vec::new();
    response
        .take(limit)
        .read_to_end(&mut body)
        .map_err(|err| BrokerError::Fetch(err.to_string()))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(BrokerError::Oversize);
    }
    Ok(HopResponse {
        status,
        location,
        content_type,
        body,
    })
}

fn finalize_body(hop: HopResponse) -> Result<Value, BrokerError> {
    if !is_text_content_type(&hop.content_type) {
        return Err(BrokerError::Binary);
    }
    if hop.body.len() > MAX_BODY_BYTES {
        return Err(BrokerError::Oversize);
    }
    let text = String::from_utf8(hop.body).map_err(|_| BrokerError::Binary)?;
    Ok(json!({
        "status": hop.status,
        "content_type": hop.content_type,
        "text": text,
    }))
}

fn is_text_content_type(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();
    if mime.is_empty() {
        return true;
    }
    let mime = mime.to_ascii_lowercase();
    mime.starts_with("text/")
        || mime == "application/json"
        || mime == "application/x-javascript"
        || mime == "application/xml"
        || mime == "application/javascript"
        || mime == "application/xhtml+xml"
        || mime.ends_with("+json")
        || mime.ends_with("+xml")
}

fn classify_reqwest(err: &reqwest::Error) -> BrokerError {
    if err.is_timeout() {
        return BrokerError::Timeout;
    }
    BrokerError::Fetch(redact_secrets(&err.to_string()))
}

fn redact_secrets(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let Some(idx) = lower.find("bearer ") else {
        return text.to_owned();
    };
    let start = idx.saturating_add("bearer ".len());
    let rest = text.get(start..).unwrap_or("");
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .unwrap_or(rest.len());
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..start]);
    out.push_str("[redacted]");
    if let Some(tail) = text.get(start.saturating_add(end)..) {
        out.push_str(tail);
    }
    out
}

fn pinned_client(url: &Url) -> Result<reqwest::blocking::Client, BrokerError> {
    drop(rustls::crypto::ring::default_provider().install_default());
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
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
            #[cfg(test)]
            if let Some(stub) = DNS_STUB.with(|cell| *cell.borrow()) {
                return resolve_from_ips(port, &stub(host));
            }
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

#[cfg(test)]
fn resolve_from_ips(port: u16, ips: &[IpAddr]) -> Result<Option<SocketAddr>, BrokerError> {
    let mut chosen = None;
    for ip in ips {
        if is_private(*ip) {
            return Err(BrokerError::Ssrf("private hosts are blocked".to_owned()));
        }
        chosen.get_or_insert(SocketAddr::new(*ip, port));
    }
    if chosen.is_none() {
        return Err(BrokerError::Ssrf("host did not resolve".to_owned()));
    }
    Ok(chosen)
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
    use super::{
        HopResponse, MAX_BODY_BYTES, deny_ssrf, get, get_inject, redact_secrets, resolve_public,
        with_dns_stub, with_hop_stub,
    };
    use crate::broker::BrokerError;
    use std::net::IpAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    #[test]
    fn dns_rebinding_to_loopback_is_denied() {
        with_dns_stub(
            |_| vec![IpAddr::from([127, 0, 0, 1])],
            || {
                let url = url::Url::parse("http://evil.example/").unwrap();
                assert!(matches!(resolve_public(&url), Err(BrokerError::Ssrf(_))));
            },
        );
    }

    #[test]
    fn dns_rebinding_mixed_public_and_private_is_denied() {
        with_dns_stub(
            |_| vec![IpAddr::from([8, 8, 8, 8]), IpAddr::from([127, 0, 0, 1])],
            || {
                let url = url::Url::parse("http://evil.example/").unwrap();
                assert!(matches!(resolve_public(&url), Err(BrokerError::Ssrf(_))));
            },
        );
    }

    #[test]
    fn redirect_to_loopback_fails_before_follow() {
        with_hop_stub(
            |url, _auth| {
                assert!(
                    !url.contains("127.0.0.1"),
                    "private hop must not be fetched: {url}"
                );
                Ok(HopResponse {
                    status: 302,
                    location: Some("http://127.0.0.1/secret".to_owned()),
                    content_type: String::new(),
                    body: Vec::new(),
                })
            },
            || {
                assert!(matches!(
                    get("https://example.invalid/start"),
                    Err(BrokerError::Ssrf(_))
                ));
            },
        );
    }

    #[test]
    fn oversize_body_is_distinguished() {
        with_hop_stub(
            |_url, _auth| {
                Ok(HopResponse {
                    status: 200,
                    location: None,
                    content_type: "text/plain".to_owned(),
                    body: vec![b'x'; MAX_BODY_BYTES.saturating_add(1)],
                })
            },
            || {
                assert!(matches!(
                    get("https://example.invalid/big"),
                    Err(BrokerError::Oversize)
                ));
            },
        );
    }

    #[test]
    fn binary_content_type_is_distinguished() {
        with_hop_stub(
            |_url, _auth| {
                Ok(HopResponse {
                    status: 200,
                    location: None,
                    content_type: "image/png".to_owned(),
                    body: vec![0x89, b'P', b'N', b'G'],
                })
            },
            || {
                assert!(matches!(
                    get("https://example.invalid/img"),
                    Err(BrokerError::Binary)
                ));
            },
        );
    }

    #[test]
    fn duckduckgo_json_content_type_is_text() {
        with_hop_stub(
            |_url, _auth| {
                Ok(HopResponse {
                    status: 202,
                    location: None,
                    content_type: "application/x-javascript".to_owned(),
                    body: br#"{"Heading":"Rust"}"#.to_vec(),
                })
            },
            || {
                let value = get("https://api.duckduckgo.com/").unwrap();
                assert_eq!(value["status"], 202);
                assert!(value["text"].as_str().unwrap().contains("Rust"));
            },
        );
    }

    #[test]
    fn timeout_is_distinguished() {
        with_hop_stub(
            |_url, _auth| Err(BrokerError::Timeout),
            || {
                assert!(matches!(
                    get("https://example.invalid/slow"),
                    Err(BrokerError::Timeout)
                ));
            },
        );
    }

    #[test]
    fn redirect_loop_is_distinguished() {
        with_hop_stub(
            |url, _auth| {
                Ok(HopResponse {
                    status: 302,
                    location: Some(format!("{url}?again=1")),
                    content_type: String::new(),
                    body: Vec::new(),
                })
            },
            || {
                assert!(matches!(
                    get("https://example.invalid/loop"),
                    Err(BrokerError::RedirectLoop)
                ));
            },
        );
    }

    #[test]
    fn injected_credential_is_not_returned() {
        with_hop_stub(
            |_url, auth| {
                assert_eq!(auth, Some("Bearer super-secret-token"));
                Ok(HopResponse {
                    status: 200,
                    location: None,
                    content_type: "text/plain".to_owned(),
                    body: b"ok".to_vec(),
                })
            },
            || {
                let value = get_inject(
                    "https://example.invalid/",
                    Some("Bearer super-secret-token"),
                )
                .unwrap();
                let dumped = value.to_string();
                assert_eq!(value["text"], "ok");
                assert!(!dumped.contains("super-secret-token"));
                assert!(!dumped.contains("Bearer"));
            },
        );
    }

    #[test]
    fn redirect_drops_authorization_on_other_host() {
        thread_local! {
            static HOPS: AtomicUsize = const { AtomicUsize::new(0) };
        }
        HOPS.with(|hops| hops.store(0, Ordering::SeqCst));
        with_hop_stub(
            |url, auth| {
                let n = HOPS.with(|hops| hops.fetch_add(1, Ordering::SeqCst));
                if n == 0 {
                    assert_eq!(auth, Some("Bearer super-secret-token"));
                    return Ok(HopResponse {
                        status: 302,
                        location: Some("https://other.invalid/next".to_owned()),
                        content_type: String::new(),
                        body: Vec::new(),
                    });
                }
                assert!(url.contains("other.invalid"));
                assert_eq!(auth, None);
                Ok(HopResponse {
                    status: 200,
                    location: None,
                    content_type: "text/plain".to_owned(),
                    body: b"ok".to_vec(),
                })
            },
            || {
                let value = get_inject(
                    "https://example.invalid/",
                    Some("Bearer super-secret-token"),
                )
                .unwrap();
                assert_eq!(value["text"], "ok");
                assert_eq!(HOPS.with(|hops| hops.load(Ordering::SeqCst)), 2);
            },
        );
    }

    #[test]
    fn redirect_drops_authorization_on_scheme_change() {
        thread_local! {
            static HOPS: AtomicUsize = const { AtomicUsize::new(0) };
        }
        HOPS.with(|hops| hops.store(0, Ordering::SeqCst));
        with_hop_stub(
            |_url, auth| {
                let n = HOPS.with(|hops| hops.fetch_add(1, Ordering::SeqCst));
                if n == 0 {
                    assert_eq!(auth, Some("Bearer super-secret-token"));
                    return Ok(HopResponse {
                        status: 302,
                        location: Some("http://example.invalid/next".to_owned()),
                        content_type: String::new(),
                        body: Vec::new(),
                    });
                }
                assert_eq!(auth, None);
                Ok(HopResponse {
                    status: 200,
                    location: None,
                    content_type: "text/plain".to_owned(),
                    body: b"ok".to_vec(),
                })
            },
            || {
                let value = get_inject(
                    "https://example.invalid/",
                    Some("Bearer super-secret-token"),
                )
                .unwrap();
                assert_eq!(value["text"], "ok");
            },
        );
    }

    #[test]
    fn redirect_drops_authorization_on_port_change() {
        thread_local! {
            static HOPS: AtomicUsize = const { AtomicUsize::new(0) };
        }
        HOPS.with(|hops| hops.store(0, Ordering::SeqCst));
        with_hop_stub(
            |_url, auth| {
                let n = HOPS.with(|hops| hops.fetch_add(1, Ordering::SeqCst));
                if n == 0 {
                    assert_eq!(auth, Some("Bearer super-secret-token"));
                    return Ok(HopResponse {
                        status: 302,
                        location: Some("https://example.invalid:8443/next".to_owned()),
                        content_type: String::new(),
                        body: Vec::new(),
                    });
                }
                assert_eq!(auth, None);
                Ok(HopResponse {
                    status: 200,
                    location: None,
                    content_type: "text/plain".to_owned(),
                    body: b"ok".to_vec(),
                })
            },
            || {
                let value = get_inject(
                    "https://example.invalid/",
                    Some("Bearer super-secret-token"),
                )
                .unwrap();
                assert_eq!(value["text"], "ok");
            },
        );
    }

    #[test]
    fn redirect_keeps_authorization_on_same_origin() {
        thread_local! {
            static HOPS: AtomicUsize = const { AtomicUsize::new(0) };
        }
        HOPS.with(|hops| hops.store(0, Ordering::SeqCst));
        with_hop_stub(
            |_url, auth| {
                let n = HOPS.with(|hops| hops.fetch_add(1, Ordering::SeqCst));
                assert_eq!(auth, Some("Bearer super-secret-token"));
                if n == 0 {
                    return Ok(HopResponse {
                        status: 302,
                        location: Some("/next".to_owned()),
                        content_type: String::new(),
                        body: Vec::new(),
                    });
                }
                Ok(HopResponse {
                    status: 200,
                    location: None,
                    content_type: "text/plain".to_owned(),
                    body: b"ok".to_vec(),
                })
            },
            || {
                let value = get_inject(
                    "https://example.invalid/start",
                    Some("Bearer super-secret-token"),
                )
                .unwrap();
                assert_eq!(value["text"], "ok");
            },
        );
    }

    #[test]
    fn redact_secrets_strips_bearer_tokens() {
        assert_eq!(
            redact_secrets("failed Bearer super-secret-token trailing"),
            "failed Bearer [redacted] trailing"
        );
    }
}
