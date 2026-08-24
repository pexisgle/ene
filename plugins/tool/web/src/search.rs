use serde_json::{Value, json};

use super::credentials::WebCredentials;
use super::html::fail;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchBackend {
    DuckDuckGo,
    Arxiv,
    Tavily,
    Exa,
}

impl SearchBackend {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "duckduckgo" | "ddg" => Ok(Self::DuckDuckGo),
            "arxiv" => Ok(Self::Arxiv),
            "tavily" => Ok(Self::Tavily),
            "exa" => Ok(Self::Exa),
            other => Err(fail(
                "backend_unconfigured",
                format!("unknown search backend {other}"),
            )),
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "duckduckgo",
            Self::Arxiv => "arxiv",
            Self::Tavily => "tavily",
            Self::Exa => "exa",
        }
    }

    fn needs_credential(self) -> bool {
        matches!(self, Self::Tavily | Self::Exa)
    }

    fn kind(self) -> &'static str {
        match self {
            Self::Arxiv => "domain",
            _ => "web",
        }
    }
}

pub(crate) fn parse_backend(raw: Option<&str>) -> Result<SearchBackend, String> {
    SearchBackend::parse(raw.unwrap_or("duckduckgo"))
}

/// Availability is a runtime property: the host injects vault credentials per
/// call and reports them here instead of hardcoding paid backends as missing.
pub(crate) fn catalog(creds: Option<&WebCredentials>) -> Value {
    json!({
        "backends": [
            cap(SearchBackend::DuckDuckGo, true, None),
            cap(SearchBackend::Arxiv, true, None),
            cap(SearchBackend::Tavily, backend_available(creds, SearchBackend::Tavily), None),
            cap(SearchBackend::Exa, backend_available(creds, SearchBackend::Exa), None),
        ],
        "default": "duckduckgo",
    })
}

fn backend_available(creds: Option<&WebCredentials>, backend: SearchBackend) -> bool {
    !backend.needs_credential() || creds.and_then(|creds| creds.for_backend(backend)).is_some()
}

fn cap(backend: SearchBackend, available: bool, code: Option<&str>) -> Value {
    json!({
        "id": backend.id(),
        "kind": backend.kind(),
        "available": available,
        "needs_credential": backend.needs_credential(),
        "code": code,
    })
}

pub(crate) fn require_available(backend: SearchBackend) -> Result<(), String> {
    if !backend.needs_credential() {
        return Ok(());
    }
    let installed =
        credentials_installed().and_then(|creds| creds.for_backend(backend).map(str::to_owned));
    if installed.is_none() {
        return Err(fail(
            "credential_missing",
            format!(
                "{} needs an API key; configure it in plugin settings",
                backend.id()
            ),
        ));
    }
    Ok(())
}

fn credentials_installed() -> Option<WebCredentials> {
    super::credentials::try_credentials()
}

pub(crate) fn parse_arxiv_atom(xml: &str) -> Vec<Value> {
    let mut results = Vec::new();
    let mut rest = xml;
    while results.len() < 8 {
        let Some(entry_at) = rest.find("<entry") else {
            break;
        };
        let after = &rest[entry_at..];
        let Some(end) = after.find("</entry>") else {
            break;
        };
        let entry = &after[..end];
        let title = atom_field(entry, "title");
        let url = atom_field(entry, "id");
        let snippet = atom_field(entry, "summary");
        if !title.is_empty() {
            results.push(json!({
                "title": title,
                "url": url,
                "snippet": snippet,
            }));
        }
        rest = after.get(end + 8..).unwrap_or("");
    }
    results
}

fn atom_field(entry: &str, name: &str) -> String {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let Some(start) = entry.find(&open) else {
        return String::new();
    };
    let after = &entry[start..];
    let Some(gt) = after.find('>') else {
        return String::new();
    };
    let rest = &after[gt + 1..];
    let Some(end) = rest.find(&close) else {
        return String::new();
    };
    collapse(&rest[..end])
}

fn collapse(text: &str) -> String {
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
    use super::super::credentials::with_credentials;
    use super::{
        SearchBackend, WebCredentials, catalog, parse_arxiv_atom, parse_backend, require_available,
    };

    fn creds(tavily: bool, exa: bool) -> WebCredentials {
        WebCredentials {
            tavily: tavily.then(|| "tvly-test".to_owned()),
            exa: exa.then(|| "exa-test".to_owned()),
        }
    }

    #[test]
    fn catalog_reflects_injected_credentials() {
        let rows = |catalog: &serde_json::Value| {
            catalog["backends"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| (row["id"].clone(), row["available"].clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(&catalog(None)),
            vec![
                ("duckduckgo".into(), true.into()),
                ("arxiv".into(), true.into()),
                ("tavily".into(), false.into()),
                ("exa".into(), false.into())
            ],
        );
        let with_both = with_credentials(creds(true, true), || catalog(Some(&creds(true, true))));
        assert_eq!(with_both["backends"][2]["available"], true);
        assert_eq!(with_both["backends"][3]["available"], true);
    }

    #[test]
    fn require_available_follows_runtime_credentials() {
        with_credentials(creds(true, false), || {
            assert!(require_available(SearchBackend::Tavily).is_ok());
            let err = require_available(SearchBackend::Exa).unwrap_err();
            assert!(err.contains("credential_missing"), "{err}");
        });
    }

    #[test]
    fn free_backends_need_no_credential() {
        assert!(require_available(SearchBackend::DuckDuckGo).is_ok());
        assert!(require_available(SearchBackend::Arxiv).is_ok());
        assert!(parse_backend(Some("arxiv")).is_ok());
    }

    #[test]
    fn arxiv_atom_reads_entries() {
        let xml = r"<feed><entry>
            <id>https://arxiv.org/abs/1234.5678</id>
            <title>A Paper</title>
            <summary>Hello world</summary>
            </entry></feed>";
        let rows = parse_arxiv_atom(xml);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["title"], "A Paper");
        assert_eq!(rows[0]["url"], "https://arxiv.org/abs/1234.5678");
        assert_eq!(rows[0]["snippet"], "Hello world");
    }
}
