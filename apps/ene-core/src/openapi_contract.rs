//! Router vs `openapi.json` path/method contract (tests only).

use std::collections::{BTreeMap, BTreeSet};

/// UI and meta routes that are not part of the documented `/api/v1` contract.
const OPENAPI_EXCLUDED: &[&str] = &["/", "/web", "/api/v1/health", "/api/v1/openapi.json"];

const HTTP_METHODS: &[&str] = &["get", "post", "put", "patch", "delete"];

type RouteTable = BTreeMap<String, BTreeSet<String>>;

fn router_contract() -> RouteTable {
    let src = include_str!("http/mod.rs");
    filter_excluded(parse_router_routes(src).expect("parse fn router route table"))
}

fn openapi_contract() -> RouteTable {
    filter_excluded(parse_openapi_paths(ene_api::openapi_json()).expect("parse openapi.json"))
}

fn filter_excluded(mut table: RouteTable) -> RouteTable {
    for path in OPENAPI_EXCLUDED {
        table.remove(*path);
    }
    table
}

fn parse_router_routes(src: &str) -> Result<RouteTable, String> {
    let body = fn_body(src, "router").ok_or_else(|| "fn router not found".to_owned())?;
    let mut table = RouteTable::new();
    let mut cursor = 0;
    while let Some(rel) = body[cursor..].find(".route(") {
        let args_start = cursor + rel + ".route(".len();
        let (args, next) = take_until(body, args_start, b'(', b')')?;
        cursor = next;
        let path = first_string_literal(&args)
            .ok_or_else(|| format!("route is missing a path string: {args}"))?;
        let mut methods = BTreeSet::new();
        for method in HTTP_METHODS {
            if has_method_call(&args, method) {
                methods.insert((*method).to_owned());
            }
        }
        if methods.is_empty() {
            return Err(format!("route {path} has no HTTP methods"));
        }
        table.entry(path).or_default().extend(methods);
    }
    if table.is_empty() {
        return Err("fn router has no .route() calls".to_owned());
    }
    Ok(table)
}

fn parse_openapi_paths(json: &str) -> Result<RouteTable, String> {
    let doc: serde_json::Value =
        serde_json::from_str(json).map_err(|err| format!("openapi.json: {err}"))?;
    let prefix = doc
        .pointer("/servers/0/url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim_end_matches('/');
    let paths = doc
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "openapi.json has no paths object".to_owned())?;
    let mut table = RouteTable::new();
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        let full = if prefix.is_empty() || path.starts_with(prefix) {
            path.clone()
        } else {
            format!("{prefix}{path}")
        };
        let mut methods = BTreeSet::new();
        for method in HTTP_METHODS {
            if item.contains_key(*method) {
                methods.insert((*method).to_owned());
            }
        }
        if methods.is_empty() {
            return Err(format!("openapi path {full} has no HTTP methods"));
        }
        table.entry(full).or_default().extend(methods);
    }
    Ok(table)
}

fn fn_body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("fn {name}(");
    let start = src.find(&needle)?;
    let brace = src[start..].find('{')? + start;
    let (_, end) = take_until(src, brace + 1, b'{', b'}').ok()?;
    Some(&src[brace + 1..end.saturating_sub(1)])
}

fn first_string_literal(src: &str) -> Option<String> {
    let start = src.find('"')?;
    let bytes = src.as_bytes();
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i = i.saturating_add(2);
            continue;
        }
        if bytes[i] == b'"' {
            return Some(src[start + 1..i].to_owned());
        }
        i += 1;
    }
    None
}

fn has_method_call(src: &str, method: &str) -> bool {
    let mut rest = src;
    while let Some(idx) = rest.find(method) {
        let before_ok = idx == 0
            || rest
                .as_bytes()
                .get(idx - 1)
                .is_some_and(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_');
        let after = &rest[idx + method.len()..];
        let after_ok = after.trim_start().starts_with('(');
        if before_ok && after_ok {
            return true;
        }
        rest = &rest[idx + method.len()..];
    }
    false
}

fn take_until(
    src: &str,
    inner_start: usize,
    open: u8,
    close: u8,
) -> Result<(String, usize), String> {
    let bytes = src.as_bytes();
    let mut depth = 1_i32;
    let mut i = inner_start;
    let mut in_string = false;
    let mut escaped = false;
    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
        } else if byte == open {
            depth += 1;
        } else if byte == close {
            depth -= 1;
            if depth == 0 {
                return Ok((src[inner_start..i].to_owned(), i + 1));
            }
        }
        i += 1;
    }
    Err("unbalanced delimiters".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_paths_match_the_http_router() {
        assert_eq!(router_contract(), openapi_contract());
    }

    #[test]
    fn extra_router_path_is_a_contract_failure() {
        let mut router = router_contract();
        router
            .entry("/api/v1/__drift_probe".to_owned())
            .or_default()
            .insert("get".to_owned());
        assert_ne!(router, openapi_contract());
    }

    #[test]
    fn missing_method_is_a_contract_failure() {
        let mut spec = openapi_contract();
        spec.get_mut("/api/v1/sessions")
            .expect("sessions is documented")
            .remove("post");
        assert_ne!(router_contract(), spec);
    }

    #[test]
    fn excluded_ui_routes_are_named() {
        assert!(OPENAPI_EXCLUDED.contains(&"/"));
        assert!(OPENAPI_EXCLUDED.contains(&"/web"));
        assert!(OPENAPI_EXCLUDED.contains(&"/api/v1/health"));
        assert!(OPENAPI_EXCLUDED.contains(&"/api/v1/openapi.json"));
        let raw_router = parse_router_routes(include_str!("http/mod.rs")).unwrap();
        assert!(raw_router.contains_key("/"));
        assert!(raw_router.contains_key("/web"));
        assert!(!router_contract().contains_key("/"));
        assert!(!router_contract().contains_key("/web"));
    }

    #[test]
    fn parser_reads_multiline_chained_methods() {
        let src = r#"
            fn router(state: AppState) -> Router {
                Router::new()
                    .route(
                        "/api/v1/sessions",
                        get(routes::list_sessions).post(routes::create_session),
                    )
                    .route("/api/v1/mcp", get(routes::get_mcp).put(routes::put_mcp))
            }
        "#;
        let table = parse_router_routes(src).unwrap();
        assert_eq!(
            table["/api/v1/sessions"],
            BTreeSet::from(["get".to_owned(), "post".to_owned()])
        );
        assert_eq!(
            table["/api/v1/mcp"],
            BTreeSet::from(["get".to_owned(), "put".to_owned()])
        );
    }
}
