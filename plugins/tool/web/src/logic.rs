use ene_plugin_ipc::ToolSpecWire;
use ene_registry::{arg_str, spec, try_host_fetch};
use serde_json::{Value, json};
use std::net::IpAddr;
use url::Url;

pub(crate) fn specs() -> Vec<ToolSpecWire> {
    vec![
        spec(
            "web.fetch",
            "Fetch a URL via HTTPS and return text",
            json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}),
            Vec::new(),
        ),
        spec(
            "web.search",
            "Search the public web (DuckDuckGo answers, HTML fallback)",
            json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}),
            Vec::new(),
        ),
    ]
}

pub(crate) fn execute(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "web.fetch" => fetch(arg_str(args, "url")?),
        "web.search" => search(arg_str(args, "query")?),
        other => Err(format!("unknown builtin {other}")),
    }
}

fn fetch(raw: &str) -> Result<Value, String> {
    deny_ssrf(raw)?;
    let payload = host_get(raw)?;
    let status = status_of(&payload);
    let content_type = str_field(&payload, "content_type");
    let body = str_field(&payload, "text");
    let text = if content_type.contains("html") {
        strip_tags(&body)
    } else {
        body
    };
    Ok(json!({ "status": status, "content_type": content_type, "text": text }))
}

fn search(query: &str) -> Result<Value, String> {
    if query.trim().is_empty() {
        return Err("query is empty".to_owned());
    }
    let mut url = deny_ssrf("https://api.duckduckgo.com/")?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "json")
        .append_pair("no_html", "1")
        .append_pair("skip_disambig", "1");
    let payload: Value = serde_json::from_str(&str_field(&host_get(url.as_str())?, "text"))
        .unwrap_or_else(|_| json!({}));
    let heading = payload
        .get("Heading")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let abstract_text = payload
        .get("AbstractText")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let abstract_url = payload
        .get("AbstractURL")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let mut results = Vec::new();
    if !abstract_text.is_empty() {
        results.push(json!({
            "title": heading,
            "url": abstract_url,
            "snippet": abstract_text,
        }));
    }
    if let Some(related) = payload.get("RelatedTopics").and_then(Value::as_array) {
        for topic in related.iter().take(8) {
            let title = topic
                .get("Text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            let topic_url = topic
                .get("FirstURL")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned();
            if !title.is_empty() {
                results.push(json!({ "title": title, "url": topic_url, "snippet": "" }));
            }
        }
    }
    if results.is_empty()
        && let Ok(html_rows) = search_html(query)
    {
        results = html_rows;
    }
    Ok(json!({ "results": results }))
}

fn search_html(query: &str) -> Result<Vec<Value>, String> {
    let mut url = deny_ssrf("https://html.duckduckgo.com/html/")?;
    url.query_pairs_mut().append_pair("q", query);
    let html = str_field(&host_get(url.as_str())?, "text");
    Ok(parse_ddg_html(&html))
}

fn host_get(raw: &str) -> Result<Value, String> {
    try_host_fetch(raw).unwrap_or_else(|| Err("web tools require the host net broker".to_owned()))
}

fn status_of(payload: &Value) -> u16 {
    payload
        .get("status")
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or(0)
}

fn str_field(payload: &Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn parse_ddg_html(html: &str) -> Vec<Value> {
    let mut results = Vec::new();
    let mut rest = html;
    while results.len() < 8 {
        let Some(idx) = rest.find("result__a") else {
            break;
        };
        rest = &rest[idx..];
        let Some(href_at) = rest.find("href=\"") else {
            break;
        };
        let after_href = &rest[href_at + 6..];
        let Some(quote) = after_href.find('"') else {
            break;
        };
        let href = decode_ddg_href(&after_href[..quote]);
        let Some(gt) = after_href.find('>') else {
            break;
        };
        let after_gt = &after_href[gt + 1..];
        let Some(lt) = after_gt.find('<') else {
            break;
        };
        let title = strip_tags(&after_gt[..lt]);
        if !title.is_empty() && !href.is_empty() {
            results.push(json!({
                "title": title,
                "url": href,
                "snippet": "",
            }));
        }
        rest = after_gt.get(lt + 1..).unwrap_or("");
    }
    results
}

fn decode_ddg_href(href: &str) -> String {
    let raw = href
        .split("uddg=")
        .nth(1)
        .and_then(|tail| tail.split('&').next())
        .unwrap_or(href);
    let decoded = percent_decode(raw);
    if decoded.starts_with("//") {
        format!("https:{decoded}")
    } else if decoded.starts_with("http://") || decoded.starts_with("https://") {
        decoded
    } else if href.starts_with("//") {
        format!("https:{href}")
    } else {
        decoded
    }
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%'
            && let Some(h1) = bytes.get(idx + 1)
            && let Some(h2) = bytes.get(idx + 2)
            && let Ok(hex) = std::str::from_utf8(&[*h1, *h2])
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            out.push(value);
            idx += 3;
            continue;
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn deny_ssrf(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|err| err.to_string())?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err("only http(s) URLs are allowed".to_owned());
    }
    let host = url.host_str().ok_or_else(|| "URL has no host".to_owned())?;
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".localhost") {
        return Err("private hosts are blocked".to_owned());
    }
    if let Ok(ip) = host.parse::<IpAddr>()
        && is_private(ip)
    {
        return Err("private hosts are blocked".to_owned());
    }
    Ok(url)
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_private(IpAddr::V4(mapped));
            }
            v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local()
        }
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    collapse_ws(&out)
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            prev_space = false;
            out.push(c);
        }
    }
    out.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::{decode_ddg_href, execute, parse_ddg_html};
    use ene_registry::with_http_fetch;
    use serde_json::json;

    #[test]
    fn parse_ddg_html_reads_result_links() {
        let html = r#"
            <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.invalid%2Ftokyo">Tokyo</a>
            <a class="result__a" href="https://example.invalid/other">Other</a>
        "#;
        let rows = parse_ddg_html(html);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["title"], "Tokyo");
        assert_eq!(rows[0]["url"], "https://example.invalid/tokyo");
        assert_eq!(rows[1]["url"], "https://example.invalid/other");
    }

    #[test]
    fn decode_ddg_href_unwraps_uddg() {
        assert_eq!(
            decode_ddg_href("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.invalid%2Fx&rut=1"),
            "https://example.invalid/x"
        );
    }

    #[test]
    fn fetch_without_host_broker_fails_closed() {
        let err = execute("web.fetch", &json!({"url":"https://example.invalid/"})).unwrap_err();
        assert!(err.contains("host net broker"), "{err}");
    }

    #[test]
    fn fetch_still_blocks_loopback_before_broker() {
        let err = execute("web.fetch", &json!({"url":"http://127.0.0.1/secret"})).unwrap_err();
        assert!(err.contains("private hosts"), "{err}");
    }

    #[test]
    fn fetch_uses_host_broker_and_strips_html() {
        let value = with_http_fetch(
            |url| {
                assert_eq!(url, "https://example.invalid/page");
                Ok(json!({
                    "status": 200,
                    "content_type": "text/html; charset=utf-8",
                    "text": "<p>Hello <b>world</b></p>"
                }))
            },
            || execute("web.fetch", &json!({"url":"https://example.invalid/page"})),
        )
        .unwrap();
        assert_eq!(value["status"], 200);
        assert_eq!(value["text"], "Hello world");
    }

    #[test]
    fn search_uses_host_broker_json() {
        let value = with_http_fetch(
            |url| {
                assert!(url.contains("api.duckduckgo.com"), "{url}");
                Ok(json!({
                    "status": 200,
                    "content_type": "application/json",
                    "text": r#"{"Heading":"Tokyo","AbstractText":"Capital","AbstractURL":"https://example.invalid/tokyo","RelatedTopics":[]}"#
                }))
            },
            || execute("web.search", &json!({"query":"tokyo"})),
        )
        .unwrap();
        assert_eq!(value["results"][0]["title"], "Tokyo");
        assert_eq!(value["results"][0]["snippet"], "Capital");
    }
}
