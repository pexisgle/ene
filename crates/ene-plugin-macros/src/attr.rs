//! Attribute parsing for the `#[tool(...)]` container attribute and the
//! `#[arg(...)]` field-level attribute on `ToolSpec` derive input.
//!
//! Uses syn v3's `parse_nested_meta` API natively — no darling dependency.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{DeriveInput, Path};

pub(crate) fn path_ident_str(path: &Path) -> String {
    path.get_ident()
        .map_or_else(|| format!("{path:?}"), ToString::to_string)
}

/// `background_capable` (bare) → `true`
/// `background_capable = true` → `true`
/// `background_capable = false` → `false`
pub(crate) fn parse_flag(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<bool> {
    if meta.input.peek(syn::Token![=]) {
        let lit: syn::LitStr = meta.value()?.parse()?;
        match lit.value().as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(syn::Error::new_spanned(lit, "expected `true` or `false`")),
        }
    } else {
        Ok(true)
    }
}

/// A comma-separated list of strings.
#[derive(Debug, Clone, Default)]
pub struct StringList(pub Vec<String>);

impl StringList {
    fn from_attr_value(s: &str) -> Self {
        Self(
            s.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
        )
    }
}

/// A semicolon-separated list of strings.
#[derive(Debug, Clone, Default)]
pub struct SemiList(pub Vec<String>);

impl SemiList {
    fn from_attr_value(s: &str) -> Self {
        Self(
            s.split(';')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
        )
    }
}

/// A field-level `#[arg(...)]` block.
///
/// All members are optional. The default is "no overrides", meaning the
/// field is included in the schema with whatever `schemars` produces
/// from the field's Rust type plus its `///` doc comment.
#[derive(Debug, Default)]
pub struct ArgAttrs {
    /// When `true`, the field is hidden from the generated JSON schema.
    pub internal: bool,

    /// Constrain this `String` (or string-typed) field to a fixed set of
    /// values. Comma-separated: `enum_values = "left, right, middle"`.
    pub enum_values: StringList,

    /// Default value to inject into the schema's `default` field.
    pub default: Option<String>,

    /// Minimum (inclusive) for numeric fields.
    pub minimum: Option<i64>,

    /// Maximum (inclusive) for numeric fields.
    pub maximum: Option<i64>,

    pub min_length: Option<usize>,

    pub max_length: Option<usize>,

    pub min_items: Option<usize>,

    pub max_items: Option<usize>,

    /// Free-form description override.
    pub description: Option<String>,

    /// When `true`, drop the field from `properties` AND `required`.
    pub hidden: bool,

    /// When `true`, the field is a stateful field (not an arg).
    pub skip: bool,
}

impl ArgAttrs {
    pub const fn is_hidden(&self) -> bool {
        self.internal || self.hidden || self.skip
    }

    pub fn from_field(f: &syn::Field) -> syn::Result<Self> {
        let mut attrs = Self::default();
        for attr in &f.attrs {
            if attr.path().is_ident("arg") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("internal") {
                        attrs.internal = true;
                    } else if meta.path.is_ident("hidden") {
                        attrs.hidden = true;
                    } else if meta.path.is_ident("skip") {
                        attrs.skip = true;
                    } else if meta.path.is_ident("enum_values") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.enum_values = StringList::from_attr_value(&s.value());
                    } else if meta.path.is_ident("default") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.default = Some(s.value());
                    } else if meta.path.is_ident("description") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.description = Some(s.value());
                    } else if meta.path.is_ident("minimum") {
                        let n: syn::LitInt = meta.value()?.parse()?;
                        attrs.minimum = Some(n.base10_parse()?);
                    } else if meta.path.is_ident("maximum") {
                        let n: syn::LitInt = meta.value()?.parse()?;
                        attrs.maximum = Some(n.base10_parse()?);
                    } else if meta.path.is_ident("min_length") {
                        let n: syn::LitInt = meta.value()?.parse()?;
                        attrs.min_length = Some(n.base10_parse()?);
                    } else if meta.path.is_ident("max_length") {
                        let n: syn::LitInt = meta.value()?.parse()?;
                        attrs.max_length = Some(n.base10_parse()?);
                    } else if meta.path.is_ident("min_items") {
                        let n: syn::LitInt = meta.value()?.parse()?;
                        attrs.min_items = Some(n.base10_parse()?);
                    } else if meta.path.is_ident("max_items") {
                        let n: syn::LitInt = meta.value()?.parse()?;
                        attrs.max_items = Some(n.base10_parse()?);
                    } else {
                        let name = path_ident_str(&meta.path);
                        return Err(syn::Error::new_spanned(
                            meta.path,
                            format!("unknown arg attribute: `{name}`"),
                        ));
                    }
                    Ok(())
                })?;
            }
        }
        Ok(attrs)
    }
}

/// Recognizes `#[tool(skip)]` standalone and `#[tool(skip, name = "…")]`
/// where `skip` is the first path segment.
pub fn has_tool_skip(field: &syn::Field) -> bool {
    for attr in &field.attrs {
        if !attr.path().is_ident("tool") {
            continue;
        }
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

#[derive(Debug, Default)]
pub struct ToolSpecAttrs {
    pub namespace: Option<String>,
    pub name: String,
    pub display_name: Option<String>,
    pub summary: String,
    pub description: Option<String>,
    pub category: String,
    pub side_effects: Option<String>,
    pub keywords_primary: StringList,
    pub keywords_secondary: StringList,
    pub keywords_domain: StringList,
    pub keywords_negative: StringList,
    pub caveats: StringList,
    pub preconditions: StringList,
    pub related: StringList,
    pub version: Option<String>,
    pub examples: SemiList,
    pub background_capable: bool,
}

impl ToolSpecAttrs {
    pub fn from_derive_input(ast: &DeriveInput) -> syn::Result<Self> {
        let mut attrs = ToolSpecAttrs::default();
        let mut found = false;

        for attr in &ast.attrs {
            if attr.path().is_ident("tool") {
                found = true;
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("namespace") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.namespace = Some(s.value());
                    } else if meta.path.is_ident("name") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.name = s.value();
                    } else if meta.path.is_ident("display_name") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.display_name = Some(s.value());
                    } else if meta.path.is_ident("summary") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.summary = s.value();
                    } else if meta.path.is_ident("description") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.description = Some(s.value());
                    } else if meta.path.is_ident("category") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.category = s.value();
                    } else if meta.path.is_ident("side_effects") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.side_effects = Some(s.value());
                    } else if meta.path.is_ident("keywords_primary") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.keywords_primary = StringList::from_attr_value(&s.value());
                    } else if meta.path.is_ident("keywords_secondary") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.keywords_secondary = StringList::from_attr_value(&s.value());
                    } else if meta.path.is_ident("keywords_domain") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.keywords_domain = StringList::from_attr_value(&s.value());
                    } else if meta.path.is_ident("keywords_negative") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.keywords_negative = StringList::from_attr_value(&s.value());
                    } else if meta.path.is_ident("caveats") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.caveats = StringList::from_attr_value(&s.value());
                    } else if meta.path.is_ident("preconditions") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.preconditions = StringList::from_attr_value(&s.value());
                    } else if meta.path.is_ident("related") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.related = StringList::from_attr_value(&s.value());
                    } else if meta.path.is_ident("version") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.version = Some(s.value());
                    } else if meta.path.is_ident("examples") {
                        let s: syn::LitStr = meta.value()?.parse()?;
                        attrs.examples = SemiList::from_attr_value(&s.value());
                    } else if meta.path.is_ident("background_capable") {
                        attrs.background_capable = parse_flag(&meta)?;
                    } else {
                        let name = path_ident_str(&meta.path);
                        return Err(syn::Error::new_spanned(
                            meta.path,
                            format!("unknown tool attribute: `{name}`"),
                        ));
                    }
                    Ok(())
                })?;
            }
        }

        if !found {
            return Err(syn::Error::new_spanned(
                ast,
                "missing `#[tool(...)]` attribute; required fields: name, summary, category",
            ));
        }
        if attrs.name.is_empty() {
            return Err(syn::Error::new_spanned(
                ast,
                "`#[tool(name = \"...\")]` is required",
            ));
        }
        if attrs.summary.is_empty() {
            return Err(syn::Error::new_spanned(
                ast,
                "`#[tool(summary = \"...\")]` is required",
            ));
        }
        if attrs.category.is_empty() {
            return Err(syn::Error::new_spanned(
                ast,
                "`#[tool(category = \"...\")]` is required",
            ));
        }
        Ok(attrs)
    }
}

impl ToolSpecAttrs {
    pub fn full_name(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{}.{}", ns, self.name),
            None => self.name.clone(),
        }
    }

    pub fn display_name_value(&self) -> String {
        self.display_name
            .clone()
            .unwrap_or_else(|| title_case(&self.name))
    }

    pub fn summary_value(&self) -> syn::Result<String> {
        if self.summary.trim().is_empty() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "`summary` must not be empty",
            ));
        }
        Ok(self.summary.clone())
    }

    pub fn description_value(&self) -> String {
        self.description
            .clone()
            .unwrap_or_else(|| self.summary.clone())
    }

    pub fn category_path(&self) -> TokenStream2 {
        path_token(&self.category, "ene_plugin_proto::ToolCategory", "Utility")
    }

    pub fn side_effects_path(&self) -> TokenStream2 {
        if let Some(s) = &self.side_effects {
            path_token(s, "ene_plugin_proto::SideEffects", "ReadOnly")
        } else {
            quote! { ::ene_plugin_proto::SideEffects::ReadOnly }
        }
    }

    /// Side effects for the LLM-facing [`ToolSpec`](::ene_plugin_proto::ToolSpec).
    ///
    /// Unlike [`Self::side_effects_path`] (which defaults to `ReadOnly` for the
    /// RAG profile), this returns `None` when the author did not declare side
    /// effects. The parallel tool-call policy treats `None` fail-closed — the
    /// tool is never parallelized — so an undeclared tool keeps the safe
    /// sequential behavior instead of being optimistically treated as read-only.
    pub fn side_effects_spec(&self) -> TokenStream2 {
        if let Some(s) = &self.side_effects {
            let path = path_token(s, "ene_plugin_proto::SideEffects", "ReadOnly");
            quote! { Some(#path) }
        } else {
            quote! { None }
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

    pub fn version_tokens(&self) -> syn::Result<TokenStream2> {
        let v = match self.version.as_deref() {
            Some(s) => parse_version(s)?,
            None => (1, 0, 0),
        };
        let (maj, min, pat) = v;
        Ok(quote! { ToolVersion::new(#maj, #min, #pat) })
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
                ::ene_plugin_proto::ToolExample {
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
        syn::Ident::new(&format!("_TOOL_ARGS_{n}"), struct_ident.span())
    }
}

fn parse_version(s: &str) -> syn::Result<(u32, u32, u32)> {
    let mut parts = s.split('.');
    let maj: u32 = parts.next().and_then(|p| p.parse().ok()).ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("malformed version: '{s}'"),
        )
    })?;
    let min = match parts.next() {
        Some(s) => s.parse().map_err(|_| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("malformed version minor: '{s}'"),
            )
        })?,
        None => 0,
    };
    let pat = match parts.next() {
        Some(s) => s.parse().map_err(|_| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("malformed version patch: '{s}'"),
            )
        })?,
        None => 0,
    };
    if parts.next().is_some() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("version has too many segments: '{s}' (expected at most 3)"),
        ));
    }
    Ok((maj, min, pat))
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

fn path_token(name: &str, default_module: &str, default_variant: &str) -> TokenStream2 {
    if name.contains("::")
        && let Ok(path) = syn::parse_str::<Path>(name)
    {
        return quote! { #path };
    }
    let mod_path: syn::Path = match syn::parse_str(default_module) {
        Ok(p) => p,
        Err(err) => return err.to_compile_error(),
    };
    let mut parts = name.split_whitespace();
    let head = parts.next().unwrap_or(default_variant);
    if let Ok(ident) = syn::parse_str::<syn::Ident>(head) {
        let known = if default_module.contains("ToolCategory") {
            &[
                "Filesystem",
                "Shell",
                "Browser",
                "App",
                "WebSearch",
                "WebFetch",
                "Utility",
                "Memory",
                "Search",
                "Meta",
            ][..]
        } else if default_module.contains("SideEffects") {
            &[
                "ReadOnly",
                "Destructive",
                "Idempotent",
                "FileSystem",
                "Network",
                "System",
                "Browser",
            ][..]
        } else {
            &[][..]
        };
        let ident_str = ident.to_string();
        if !known.is_empty() && !known.iter().any(|&k| k == ident_str) {
            let msg = format!(
                "unknown {default_module} variant '{ident}'; expected one of: {}",
                known.join(", ")
            );
            return syn::Error::new_spanned(ident, msg).to_compile_error();
        }
        if ident == "ReadOnly" || ident == "Destructive" || ident == "Idempotent" {
            return quote! { #mod_path::#ident };
        }
        if name.contains('{') {
            let combined = format!("{default_module} :: {name}");
            let fallback = quote! { #mod_path::#default_variant };
            let stream: TokenStream2 = combined.parse().unwrap_or(fallback);
            return stream;
        }
        return quote! { #mod_path::#ident };
    }
    let fallback_ident = syn::Ident::new(default_variant, proc_macro2::Span::call_site());
    let stream: TokenStream2 = syn::parse_str(name).unwrap_or_else(|_| quote! { #fallback_ident });
    stream
}
