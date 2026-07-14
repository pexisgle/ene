//! Attribute parsing for the `#[tool(...)]` container attribute and the
//! `#[arg(...)]` field-level attribute on `ToolSpec` derive input.

use darling::{FromDeriveInput, FromField, FromMeta};
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::Path;

/// A comma-separated list of strings, parsed from a single `FromMeta` value.
#[derive(Debug, Clone, Default)]
pub struct StringList(pub Vec<String>);

impl FromMeta for StringList {
    fn from_string(value: &str) -> darling::Result<Self> {
        Ok(Self(
            value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        ))
    }
}

/// A semicolon-separated list of strings, parsed from a single `FromMeta` value.
#[derive(Debug, Clone, Default)]
pub struct SemiList(pub Vec<String>);

impl FromMeta for SemiList {
    fn from_string(value: &str) -> darling::Result<Self> {
        Ok(Self(
            value
                .split(';')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        ))
    }
}

/// A field-level `#[arg(...)]` block.
///
/// All members are optional. The default is "no overrides", meaning the
/// field is included in the schema with whatever `schemars` produces
/// from the field's Rust type plus its `///` doc comment.
#[derive(Debug, Default, FromField)]
#[darling(attributes(arg))]
pub struct ArgAttrs {
    /// When `true`, the field is hidden from the generated JSON schema.
    /// Use this for fields that are set by the host and not by the LLM
    /// (e.g. `_user_answers` on a re-invocation path).
    #[darling(default)]
    pub internal: bool,

    /// Constrain this `String` (or string-typed) field to a fixed set of
    /// values. Comma-separated: `enum_values = "left, right, middle"`.
    #[darling(default)]
    pub enum_values: StringList,

    /// Default value to inject into the schema's `default` field, as a
    /// JSON literal: `default = "left"`, `default = "42"`, etc.
    #[darling(default)]
    pub default: Option<String>,

    /// Minimum (inclusive) for numeric fields.
    #[darling(default)]
    pub minimum: Option<i64>,

    /// Maximum (inclusive) for numeric fields.
    #[darling(default)]
    pub maximum: Option<i64>,

    /// Minimum length for string fields.
    #[darling(default)]
    pub min_length: Option<usize>,

    /// Maximum length for string fields.
    #[darling(default)]
    pub max_length: Option<usize>,

    /// Minimum number of array items.
    #[darling(default)]
    pub min_items: Option<usize>,

    /// Maximum number of array items.
    #[darling(default)]
    pub max_items: Option<usize>,

    /// Free-form description override. If unset, the `///` doc comment on
    /// the field is used (and schemars puts it in `description`).
    #[darling(default)]
    pub description: Option<String>,

    /// When `true`, drop the field from `properties` AND `required` in the
    /// generated schema. Alias for `internal` kept for readability.
    #[darling(default)]
    pub hidden: bool,

    /// When `true`, the field is a stateful field (not an arg). It is
    /// hidden from the schema and NOT deserialized from JSON. Instead,
    /// it is copied from `self` in the generated `execute()` method.
    /// Use `#[serde(skip, default = "...")]` alongside this.
    #[darling(default)]
    pub skip: bool,
}

impl ArgAttrs {
    pub const fn is_hidden(&self) -> bool {
        self.internal || self.hidden || self.skip
    }
}

/// Check if a field has `#[tool(skip)]`. Recognizes
/// `#[tool(skip)]` standalone and `#[tool(skip, name = "…")]`
/// where `skip` is the first path segment.
pub fn has_tool_skip(field: &syn::Field) -> bool {
    for attr in &field.attrs {
        if !attr.path().is_ident("tool") {
            continue;
        }
        // Match the `skip` form (path) regardless of any
        // other arguments; `parse_nested_meta` is
        // `parse_args_with` and tolerates the trailing
        // `, name = "…", …` we want to allow alongside.
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                // Mark by panicking with a special
                // payload? That would abort the parse;
                // instead, signal via a side channel by
                // mutating a thread_local — no, simpler:
                // detect at the outer level by
                // inspecting the path again.
            }
            Ok(())
        });
        // Re-parse the attribute as a punctuated
        // list of nested metas so we can check the
        // first path segment. parse_args_with into
        // Punctuated<Meta, Comma> gives us the
        // comma-separated form the user wrote.
        if let Ok(list) = attr.parse_args_with(
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        ) && let Some(first) = list.first()
            && first.path().is_ident("skip")
        {
            return true;
        }
    }
    false
}

#[derive(Debug, FromDeriveInput, Default)]
#[darling(attributes(tool))]
pub struct ToolSpecAttrs {
    /// Optional namespace. When set with `name`, the final name is
    /// `"{namespace}.{name}"`. When omitted, `name` is the full name.
    #[darling(default)]
    pub namespace: Option<String>,

    /// Required: short action name (e.g. "read") OR full name when no
    /// `namespace` is set.
    pub name: String,

    /// Human-friendly display name. Defaults to Title-Case of `name`.
    #[darling(default)]
    pub display_name: Option<String>,

    /// Required: one-line summary used for embedding.
    pub summary: String,

    /// Optional full description. Defaults to `summary`.
    #[darling(default)]
    pub description: Option<String>,

    /// Tool category. Either a bare identifier (e.g. "Filesystem") or a
    /// fully qualified path (e.g. "`::ene_tool_proto::ToolCategory::Filesystem`").
    pub category: String,

    /// Side effects expression. Can be a bare variant or fully qualified.
    #[darling(default)]
    pub side_effects: Option<String>,

    /// Primary keywords (high embedding weight). Comma-separated.
    #[darling(default)]
    pub keywords_primary: StringList,

    /// Secondary keywords (mid embedding weight). Comma-separated.
    #[darling(default)]
    pub keywords_secondary: StringList,

    /// Domain tags. Comma-separated.
    #[darling(default)]
    pub keywords_domain: StringList,

    /// Negative keywords. Comma-separated.
    #[darling(default)]
    pub keywords_negative: StringList,

    /// Free-form caveats. Comma-separated.
    #[darling(default)]
    pub caveats: StringList,

    /// Preconditions that must hold before invocation. Comma-separated.
    #[darling(default)]
    pub preconditions: StringList,

    /// Related tool names. Comma-separated.
    #[darling(default)]
    pub related: StringList,

    /// Optional semantic version. Default: 1.0.0.
    #[darling(default)]
    pub version: Option<String>,

    /// Examples, each expressed as a `ToolExample` literal. Format:
    /// `examples = "desc1|input1|output1; desc2|input2"`.
    #[darling(default)]
    pub examples: SemiList,
}

impl ToolSpecAttrs {
    pub fn full_name(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{}.{}", ns, self.name),
            None => self.name.clone(),
        }
    }

    pub fn display_name_value(&self, _default: String) -> String {
        self.display_name
            .clone()
            .unwrap_or_else(|| title_case(&self.name))
    }

    pub fn summary_value(&self) -> darling::Result<String> {
        if self.summary.trim().is_empty() {
            return Err(darling::Error::custom("`summary` must not be empty"));
        }
        Ok(self.summary.clone())
    }

    pub fn description_value(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| self.summary.clone())
    }

    pub fn category_path(&self) -> TokenStream2 {
        path_token(&self.category, "ene_tool_proto::ToolCategory")
    }

    pub fn side_effects_path(&self) -> TokenStream2 {
        if let Some(s) = &self.side_effects {
            path_token(s, "ene_tool_proto::SideEffects")
        } else {
            quote! { ::ene_tool_proto::SideEffects::ReadOnly }
        }
    }

    pub fn keywords_list(&self, kind: &str) -> Vec<String> {
        match kind {
            "primary" => self.keywords_primary.0.clone(),
            "secondary" => self.keywords_secondary.0.clone(),
            "domain" => self.keywords_domain.0.clone(),
            "negative" => self.keywords_negative.0.clone(),
            _ => Vec::new(),
        }
    }

    pub fn string_list(&self, kind: &str) -> Vec<String> {
        match kind {
            "caveats" => self.caveats.0.clone(),
            "preconditions" => self.preconditions.0.clone(),
            _ => Vec::new(),
        }
    }

    pub fn related_list(&self) -> Vec<String> {
        self.related.0.clone()
    }

    pub fn version_tokens(&self) -> TokenStream2 {
        let v = self
            .version
            .as_deref()
            .and_then(parse_version)
            .unwrap_or((1, 0, 0));
        let (maj, min, pat) = v;
        quote! { ToolVersion::new(#maj, #min, #pat) }
    }

    pub fn examples_value(&self) -> TokenStream2 {
        if self.examples.0.is_empty() {
            return quote! { ::std::vec::Vec::new() };
        }
        let items = self.examples.0.iter().map(|raw: &String| {
            let parts: Vec<&str> = raw.split('|').collect();
            let desc = parts.first().copied().unwrap_or("");
            let input_str = parts.get(1).copied().unwrap_or("");
            let output_str = parts.get(2).copied();
            let output_expr = if let Some(s) = output_str { quote! { ::std::option::Option::Some(#s.to_string()) } } else { quote! { ::std::option::Option::None } };
            quote! {
                ::ene_tool_proto::ToolExample {
                    description: #desc.to_string(),
                    input: {
                        let __s: &str = #input_str;
                        match ::serde_json::from_str::<::serde_json::Value>(__s) {
                            ::std::result::Result::Ok(__v) => __v,
                            ::std::result::Result::Err(_) => ::serde_json::Value::String(__s.to_string()),
                        }
                    },
                    output: #output_expr,
                }
            }
        });
        quote! { ::std::vec![ #(#items),* ] }
    }

    pub fn args_const_ident(&self, struct_ident: &syn::Ident) -> syn::Ident {
        let n = self.full_name().to_uppercase().replace('.', "_");
        let _ = struct_ident;
        syn::Ident::new(&format!("_TOOL_ARGS_{n}"), struct_ident.span())
    }
}

fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('.');
    let maj = parts.next()?.parse().ok()?;
    // Default missing minor and patch segments to 0
    // rather than silently returning None (which
    // would cause the macro to fall back to (1,0,0)
    // for "1.2" — almost certainly not what the user
    // meant). Reject more than 3 parts.
    let min = match parts.next() {
        Some(s) => s.parse().ok()?,
        None => 0,
    };
    let pat = match parts.next() {
        Some(s) => s.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((maj, min, pat))
}

fn title_case(s: &str) -> String {
    s.split(['_', '.'])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut chars = p.chars();
            if let Some(first_char) = chars.next() {
                let first = first_char.to_ascii_uppercase();
                let rest: String = chars.collect();
                format!("{first}{rest}")
            } else {
                String::new()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn path_token(name: &str, default_module: &str) -> TokenStream2 {
    if name.contains("::")
        && let Ok(path) = syn::parse_str::<Path>(name)
    {
        return quote! { #path };
    }
    // Parse the default module path once. A failure here means the proc-macro
    // itself is buggy (the `default_module` literals are hardcoded in the
    // call sites), so we surface a proper compile error instead of panicking
    // — proc-macro panics produce the unhelpful "proc macro panicked" message.
    let mod_path: syn::Path = match syn::parse_str(default_module) {
        Ok(p) => p,
        Err(err) => return err.to_compile_error(),
    };
    let mut parts = name.split_whitespace();
    let head = parts.next().unwrap_or("ReadOnly");
    if let Ok(ident) = syn::parse_str::<syn::Ident>(head) {
        if ident == "ReadOnly" || ident == "Destructive" || ident == "Idempotent" {
            return quote! { #mod_path::#ident };
        }
        if name.contains('{') {
            let combined = format!("{default_module} :: {name}");
            let stream: TokenStream2 = combined
                .parse()
                .unwrap_or_else(|_| quote! { #mod_path::ReadOnly });
            return stream;
        }
        return quote! { #mod_path::#ident };
    }
    let stream: TokenStream2 = syn::parse_str(name).unwrap_or_else(|_| quote! { ReadOnly });
    stream
}
