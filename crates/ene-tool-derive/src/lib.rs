//! # ene-tool-derive
//!
//! Proc-macro derive crate that generates `ToolSpec` / `ActionSpec` implementations
//! from declarative attributes on tool argument structs.
//!
//! See `docs/tools/derive-macro.md` for the full attribute reference.

use darling::FromDeriveInput;
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{DeriveInput, GenericArgument, PathArguments, Type, parse_macro_input};

mod attr;
mod codegen;
mod util;

use attr::ToolSpecAttrs;

/// Derive macro that generates a `ToolSpec::spec()` impl on the args struct.
///
/// Reads from `#[tool(...)]` container attributes and `///` doc comments.
/// Also requires `JsonSchema` and `Deserialize` to be derived by the user so
/// that JSON Schema generation works.
#[proc_macro_derive(ToolSpec, attributes(tool, arg, tool_meta))]
pub fn derive_tool_spec(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    expand_tool_spec(ast)
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

fn expand_tool_spec(ast: DeriveInput) -> syn::Result<TokenStream2> {
    let attrs = ToolSpecAttrs::from_derive_input(&ast)
        .map_err(|e| syn::Error::new_spanned(&ast, e.to_string()))?;
    let ident = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let data = match &ast.data {
        syn::Data::Struct(s) => s,
        _ => {
            return Err(syn::Error::new_spanned(
                &ast,
                "ToolSpec can only be derived on structs",
            ));
        }
    };
    let fields = match &data.fields {
        syn::Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(
                &ast,
                "ToolSpec requires a struct with named fields",
            ));
        }
    };

    let tool_name_value = attrs.tool_name_value()?;
    let tool_name_str = attrs.full_name();
    let display_name = attrs.display_name_value(ident.to_string());
    let summary = attrs.summary_value()?;
    let description = attrs.description_value();
    let category_path = attrs.category_path();
    let side_effects_path = attrs.side_effects_path();
    let keywords_primary = attrs.keywords_list("primary");
    let keywords_secondary = attrs.keywords_list("secondary");
    let keywords_domain = attrs.keywords_list("domain");
    let keywords_negative = attrs.keywords_list("negative");
    let examples_json = attrs.examples_value();
    let caveats = attrs.string_list("caveats");
    let preconditions = attrs.string_list("preconditions");
    let related = attrs.related_list();
    let version = attrs.version_tokens();

    let field_descriptions = collect_field_descriptions(fields);
    let args_ident = attrs.args_const_ident(ident);

    let _ = field_descriptions; // Currently unused; reserved for future use
    let _ = tool_name_value; // Currently unused; reserved for error reporting

    let output = quote! {
        impl #impl_generics #ident #ty_generics #where_clause {
            /// Canonical tool name (e.g. "filesystem.read"). Use this with
            /// `ToolName::new(...)` for `call_tool` dispatch.
            pub const TOOL_NAME: &'static str = #tool_name_str;

            /// Short human-friendly display name.
            pub const DISPLAY_NAME: &'static str = #display_name;

            /// One-line summary used as the primary embedding field.
            pub const SUMMARY: &'static str = #summary;

            /// Construct a `ToolSpec` for this args type.
            pub fn spec() -> ::ene_tool_proto::ToolSpec {
                use ::ene_tool_proto::{ToolSpec, ToolName, ToolVersion, ToolExample, ToolCategory, KeywordSet, SideEffects};
                use ::schemars::JsonSchema as _;
                use ::serde_json::json;

                let schema = {
                    let g = ::schemars::SchemaGenerator::default();
                    let s = g.into_root_schema_for::<Self>();
                    ::serde_json::to_value(s).unwrap_or_else(|_| json!({"type": "object"}))
                };

                let name = ToolName::new(#tool_name_str);
                let version = #version;

                ToolSpec {
                    name,
                    version,
                    display_name: #display_name.to_string(),
                    summary: #summary.to_string(),
                    description: #description.to_string(),
                    category: #category_path,
                    keywords: KeywordSet {
                        primary: vec![#(#keywords_primary.to_string()),*],
                        secondary: vec![#(#keywords_secondary.to_string()),*],
                        domain: vec![#(#keywords_domain.to_string()),*],
                        negative: vec![#(#keywords_negative.to_string()),*],
                    },
                    parameters: schema,
                    examples: #examples_json,
                    caveats: vec![#(#caveats.to_string()),*],
                    side_effects: #side_effects_path,
                    preconditions: vec![#(#preconditions.to_string()),*],
                    related: vec![#(::ene_tool_proto::ToolName::new(#related.to_string())),*],
                }
            }
        }

        // Keep args identifier available for downstream macros.
        #[allow(dead_code)]
        const #args_ident: () = ();
    };

    let _ = field_descriptions; // Currently unused; reserved for future use
    let _ = tool_name_value; // Currently unused; reserved for error reporting
    Ok(output)
}

fn collect_field_descriptions(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> Vec<TokenStream2> {
    fields
        .iter()
        .filter_map(|f| {
            let desc = util::doc_comment_text(&f.attrs);
            if desc.is_empty() {
                None
            } else {
                let name = f.ident.as_ref()?.to_string();
                Some(quote! { (#name, #desc) })
            }
        })
        .collect()
}

#[allow(dead_code)]
fn inner_type<'a>(ty: &'a Type) -> Option<&'a Type> {
    if let Type::Path(tp) = ty
        && let Some(last) = tp.path.segments.last()
        && let PathArguments::AngleBracketed(args) = &last.arguments
    {
        return args.args.iter().find_map(|arg| {
            if let GenericArgument::Type(inner) = arg {
                Some(inner)
            } else {
                None
            }
        });
    }
    None
}

#[allow(dead_code)]
fn ident_name(ident: &syn::Ident) -> String {
    ident.to_string()
}

#[allow(dead_code)]
fn format_ident_str(s: &str) -> syn::Ident {
    format_ident!("{}", s)
}
