use serde_json::{Value, json};

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

pub(crate) fn catalog() -> Value {
    json!({
        "backends": [
            cap(SearchBackend::DuckDuckGo, true, None),
            cap(SearchBackend::Arxiv, true, None),
            cap(
                SearchBackend::Tavily,
                false,
                Some("credential_missing"),
            ),
            cap(SearchBackend::Exa, false, Some("credential_missing")),
        ],
        "default": "duckduckgo",
    })
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
    if backend.needs_credential() {
        return Err(fail(
            "credential_missing",
            format!(
                "{} needs a vault credential; it is not selected without host policy",
                backend.id()
            ),
        ));
    }
    Ok(())
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
    use super::{SearchBackend, catalog, parse_arxiv_atom, parse_backend, require_available};

    #[test]
    fn catalog_lists_ddg_and_paid_gaps() {
        let value = catalog();
        let rows = value["backends"].as_array().unwrap();
        assert_eq!(rows[0]["id"], "duckduckgo");
        assert_eq!(rows[0]["available"], true);
        assert_eq!(rows[2]["id"], "tavily");
        assert_eq!(rows[2]["available"], false);
        assert_eq!(rows[2]["code"], "credential_missing");
        assert!(require_available(SearchBackend::Tavily).is_err());
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
