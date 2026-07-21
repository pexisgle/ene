//! # ene-i18n-macros
//!
//! Proc-macro that generates type-safe i18n accessor functions from Fluent
//! `.ftl` translation files, replacing the previous `build.rs` code generation
//! for the desktop app (#246).
//!
//! The [`i18n_keys!`] macro parses the English (fallback) and Japanese
//! catalogs with `fluent-syntax`, verifies that their message keys and
//! placeholder parameter sets agree, and emits one `#[must_use] pub fn` per
//! message. Messages without placeholders become `fn() -> String`; messages
//! with `{ $param }` placeholders become functions with a typed argument per
//! parameter.
//!
//! Compared to the old `build.rs` approach this gives:
//!
//! - **EN/JA consistency checking** — a key present in one catalog but not the
//!   other (or a mismatched parameter set) is a compile error.
//! - **A real FTL parser** — attributes, selectors, and multi-line values are
//!   handled by `fluent-syntax` rather than naive line matching.
//! - **Complete keyword handling** — Rust keywords become raw identifiers
//!   (`r#type`) instead of a hand-maintained allowlist.
//! - **Parameterized messages** — `{ $error }` generates `fn(error: ...)`.
//! - **Correct rebuild tracking** — the generated code embeds `include_str!`
//!   of every catalog, so Cargo re-runs the macro when any of them changes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{LitStr, Token, parse_macro_input};

/// Functional macro that generates i18n accessor functions from `.ftl` files.
///
/// # Usage
///
/// ```ignore
/// mod i18n {
///     use super::loader;
///     ene_i18n_macros::i18n_keys!(
///         "i18n/en-US/ene_desktop.ftl",
///         "i18n/ja/ene_desktop.ftl"
///     );
/// }
/// ```
///
/// Paths are resolved relative to the invoking crate's `CARGO_MANIFEST_DIR`.
/// The first path is treated as the fallback (English) catalog and is the
/// source of truth for the generated key set; every additional catalog must
/// contain exactly the same keys with the same placeholder parameters.
///
/// The generated functions call `i18n_embed_fl::fl!(loader(), "<key>", ...)`
/// and therefore require a `loader()` function returning a
/// `&'static FluentLanguageLoader` to be in scope.
#[proc_macro]
pub fn i18n_keys(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as I18nKeysArgs);
    expand(&args)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Parsed macro arguments: one or more `.ftl` file paths.
struct I18nKeysArgs {
    paths: Vec<LitStr>,
}

impl Parse for I18nKeysArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let parsed = Punctuated::<LitStr, Token![,]>::parse_terminated(input)?;
        let paths: Vec<LitStr> = parsed.into_iter().collect();
        if paths.is_empty() {
            return Err(input.error("i18n_keys! requires at least one .ftl path"));
        }
        Ok(Self { paths })
    }
}

/// A parsed catalog: its messages keyed by FTL identifier, in source order.
struct Catalog {
    /// Human-readable label used in diagnostics (e.g. `en-US`).
    label: String,
    /// Message id → extracted placeholder parameters, preserving first-seen
    /// order within a message.
    messages: BTreeMap<String, Vec<String>>,
}

/// Expand the macro into the generated accessor functions.
fn expand(args: &I18nKeysArgs) -> syn::Result<TokenStream2> {
    let first_span = args
        .paths
        .first()
        .map_or_else(proc_macro2::Span::call_site, LitStr::span);
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").map_err(|e| {
        syn::Error::new(
            first_span,
            format!("i18n_keys! could not read CARGO_MANIFEST_DIR: {e}"),
        )
    })?;
    let base = PathBuf::from(manifest_dir);

    let mut resolved: Vec<(LitStr, PathBuf)> = Vec::with_capacity(args.paths.len());
    for lit in &args.paths {
        let value = lit.value();
        let rel = Path::new(value.as_str());
        let abs = if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            base.join(rel)
        };
        resolved.push((lit.clone(), abs));
    }

    let mut catalogs: Vec<Catalog> = Vec::with_capacity(resolved.len());
    for (lit, abs) in &resolved {
        let source = std::fs::read_to_string(abs).map_err(|e| {
            syn::Error::new(
                lit.span(),
                format!("i18n_keys! could not read {}: {e}", abs.display()),
            )
        })?;
        let label = catalog_label(abs);
        let messages = parse_catalog(&source).map_err(|e| {
            syn::Error::new(
                lit.span(),
                format!("i18n_keys! failed to parse {label}: {e}"),
            )
        })?;
        catalogs.push(Catalog { label, messages });
    }

    // The first catalog is the fallback / source of truth.
    let Some((fallback, others)) = catalogs.split_first() else {
        return Err(syn::Error::new(
            first_span,
            "i18n_keys! requires at least one .ftl path",
        ));
    };
    for other in others {
        verify_consistency(fallback, other)?;
    }

    let fns = generate_functions(fallback);

    // Embed every catalog via include_str! so Cargo tracks them and re-runs
    // the macro when any translation file changes. The constants are unused at
    // runtime; they exist purely for dependency tracking.
    let tracking: Vec<TokenStream2> = resolved
        .iter()
        .map(|(_, abs)| {
            let path_str = abs.display().to_string();
            quote! {
                #[doc(hidden)]
                #[allow(dead_code)]
                const _: &str = include_str!(#path_str);
            }
        })
        .collect();

    Ok(quote! {
        #(#tracking)*
        #(#fns)*
    })
}

/// Derive a short diagnostic label from a catalog path.
///
/// Prefers the parent directory name (`en-US`, `ja`) and falls back to the
/// file stem.
fn catalog_label(path: &Path) -> String {
    let from_parent = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .map(ToOwned::to_owned);
    if let Some(label) = from_parent {
        return label;
    }
    path.file_stem()
        .and_then(|n| n.to_str())
        .map_or_else(|| path.display().to_string(), ToOwned::to_owned)
}

/// Parse an FTL source string into a map of message id → placeholder params.
fn parse_catalog(source: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let resource = fluent_syntax::parser::parse(source).map_err(|(_, errors)| {
        errors
            .iter()
            .map(|e| format!("{e:?}"))
            .collect::<Vec<_>>()
            .join("; ")
    })?;

    let mut messages = BTreeMap::new();
    for entry in resource.body {
        let fluent_syntax::ast::Entry::Message(message) = entry else {
            continue;
        };
        let params = collect_placeholders(message.value.as_ref(), &message.attributes);
        messages.insert(message.id.name.to_string(), params);
    }
    Ok(messages)
}

/// Collect the set of `{ $param }` placeholder names referenced by a message's
/// value and attributes, in first-seen order.
fn collect_placeholders(
    value: Option<&fluent_syntax::ast::Pattern<&str>>,
    attributes: &[fluent_syntax::ast::Attribute<&str>],
) -> Vec<String> {
    let mut params: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();

    let mut visit_pattern = |pattern: &fluent_syntax::ast::Pattern<&str>| {
        for element in &pattern.elements {
            if let fluent_syntax::ast::PatternElement::Placeable { expression } = element {
                collect_expression_placeholders(expression, &mut params, &mut seen);
            }
        }
    };

    if let Some(pattern) = value {
        visit_pattern(pattern);
    }
    for attr in attributes {
        visit_pattern(&attr.value);
    }

    params
}

/// Recursively collect `$variable` references from an expression, including
/// inside selectors and nested placeables.
fn collect_expression_placeholders(
    expression: &fluent_syntax::ast::Expression<&str>,
    out: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    match expression {
        fluent_syntax::ast::Expression::Select { selector, variants } => {
            collect_inline_placeholders(selector, out, seen);
            for variant in variants {
                for element in &variant.value.elements {
                    if let fluent_syntax::ast::PatternElement::Placeable { expression } = element {
                        collect_expression_placeholders(expression, out, seen);
                    }
                }
            }
        }
        fluent_syntax::ast::Expression::Inline(inline) => {
            collect_inline_placeholders(inline, out, seen);
        }
    }
}

/// Collect `$variable` references from an inline expression, recursing into
/// nested placeables (e.g. function call arguments).
fn collect_inline_placeholders(
    inline: &fluent_syntax::ast::InlineExpression<&str>,
    out: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    match inline {
        fluent_syntax::ast::InlineExpression::VariableReference { id } => {
            let name = id.name.to_string();
            if seen.insert(name.clone()) {
                out.push(name);
            }
        }
        fluent_syntax::ast::InlineExpression::Placeable { expression } => {
            collect_expression_placeholders(expression, out, seen);
        }
        fluent_syntax::ast::InlineExpression::FunctionReference { arguments, .. } => {
            for arg in &arguments.positional {
                collect_inline_placeholders(arg, out, seen);
            }
            for named in &arguments.named {
                collect_inline_placeholders(&named.value, out, seen);
            }
        }
        _ => {}
    }
}

/// Verify that two catalogs agree on keys and per-key parameter sets.
fn verify_consistency(fallback: &Catalog, other: &Catalog) -> syn::Result<()> {
    let mut problems: Vec<String> = Vec::new();

    for (key, params) in &fallback.messages {
        match other.messages.get(key) {
            None => {
                problems.push(format!(
                    "key `{key}` exists in {} but is missing from {}",
                    fallback.label, other.label
                ));
            }
            Some(other_params) if params != other_params => {
                problems.push(format!(
                    "key `{key}` has parameters {params:?} in {} but {other_params:?} in {}",
                    fallback.label, other.label
                ));
            }
            Some(_) => {}
        }
    }

    for key in other.messages.keys() {
        if !fallback.messages.contains_key(key) {
            problems.push(format!(
                "key `{key}` exists in {} but is missing from {}",
                other.label, fallback.label
            ));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!(
                "i18n_keys! catalog mismatch between {} and {}:\n  - {}",
                fallback.label,
                other.label,
                problems.join("\n  - ")
            ),
        ))
    }
}

/// Generate one accessor function per message in the fallback catalog.
fn generate_functions(catalog: &Catalog) -> Vec<TokenStream2> {
    catalog
        .messages
        .iter()
        .map(|(key, params)| generate_function(key, params))
        .collect()
}

/// Generate a single accessor function for a message.
fn generate_function(key: &str, params: &[String]) -> TokenStream2 {
    let fn_name = sanitize_ident(key);

    if params.is_empty() {
        quote! {
            #[must_use]
            pub fn #fn_name() -> ::std::string::String {
                i18n_embed_fl::fl!(loader(), #key)
            }
        }
    } else {
        let arg_names: Vec<proc_macro2::Ident> = params.iter().map(|p| sanitize_ident(p)).collect();
        // The Fluent placeholder keys are passed verbatim (they may contain
        // `-`, which is not a valid Rust identifier), so we use the `fl!`
        // HashMap form rather than the named-argument form.
        let arg_keys: Vec<&str> = params.iter().map(String::as_str).collect();

        quote! {
            #[must_use]
            pub fn #fn_name(
                #(#arg_names: impl ::std::convert::Into<::fluent_bundle::FluentValue<'static>>),*
            ) -> ::std::string::String {
                let mut args: ::std::collections::HashMap<
                    &str,
                    ::fluent_bundle::FluentValue<'static>,
                > = ::std::collections::HashMap::new();
                #(args.insert(#arg_keys, #arg_names.into());)*
                i18n_embed_fl::fl!(loader(), #key, args)
            }
        }
    }
}

/// Convert an FTL identifier (e.g. `chat-window-title`) into a valid Rust
/// function identifier, using a raw identifier for reserved keywords.
fn sanitize_ident(ftl_key: &str) -> proc_macro2::Ident {
    let snake = ftl_key.replace('-', "_");
    if is_rust_keyword(&snake) {
        format_ident!("r#{snake}")
    } else {
        format_ident!("{snake}")
    }
}

/// Whether a `snake_case` name collides with a Rust reserved keyword.
///
/// Covers the strict and reserved keyword sets so generated code always
/// compiles regardless of the FTL key names.
fn is_rust_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
            | "union"
    )
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "unit tests use expect/expect_err for concise assertions"
    )]

    use super::*;

    #[test]
    fn parse_catalog_extracts_keys_and_params() {
        let source = "\
plain = Hello
with-params = Failed: { $error }
multi-params = { $count } items in { $path }
";
        let catalog = parse_catalog(source).expect("valid FTL");
        assert_eq!(catalog.get("plain"), Some(&vec![]));
        assert_eq!(catalog.get("with-params"), Some(&vec!["error".to_string()]));
        assert_eq!(
            catalog.get("multi-params"),
            Some(&vec!["count".to_string(), "path".to_string()])
        );
    }

    #[test]
    fn parse_catalog_dedups_repeated_params() {
        let source = "repeat = { $x } and { $x } again\n";
        let catalog = parse_catalog(source).expect("valid FTL");
        assert_eq!(catalog.get("repeat"), Some(&vec!["x".to_string()]));
    }

    #[test]
    fn parse_catalog_ignores_comments_and_terms() {
        let source = "\
# a comment
-brand = Firefox
real-key = Value
";
        let catalog = parse_catalog(source).expect("valid FTL");
        assert!(!catalog.contains_key("-brand"));
        assert!(catalog.contains_key("real-key"));
    }

    #[test]
    fn sanitize_ident_handles_keywords_and_dashes() {
        assert_eq!(
            sanitize_ident("chat-window-title").to_string(),
            "chat_window_title"
        );
        assert_eq!(sanitize_ident("type").to_string(), "r#type");
        assert_eq!(sanitize_ident("loop").to_string(), "r#loop");
    }

    fn catalog_from(label: &str, source: &str) -> Catalog {
        Catalog {
            label: label.to_string(),
            messages: parse_catalog(source).expect("valid FTL"),
        }
    }

    #[test]
    fn verify_consistency_accepts_matching_catalogs() {
        let en = catalog_from("en-US", "a = A\nb = { $x }\n");
        let ja = catalog_from("ja", "a = えー\nb = { $x }\n");
        assert!(verify_consistency(&en, &ja).is_ok());
    }

    #[test]
    fn verify_consistency_rejects_missing_key() {
        let en = catalog_from("en-US", "a = A\nb = B\n");
        let ja = catalog_from("ja", "a = えー\n");
        let err = verify_consistency(&en, &ja).expect_err("should fail");
        assert!(err.to_string().contains("missing from ja"));
    }

    #[test]
    fn verify_consistency_rejects_param_mismatch() {
        let en = catalog_from("en-US", "msg = { $x }\n");
        let ja = catalog_from("ja", "msg = { $y }\n");
        let err = verify_consistency(&en, &ja).expect_err("should fail");
        assert!(err.to_string().contains("parameters"));
    }
}
