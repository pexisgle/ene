//! # ene-plugin-macros
//!
//! Proc-macro crate for plugin authoring. Generates `ToolSpec` / `ToolRagProfile`
//! implementations and `ToolAction` glue code from declarative attributes on
//! tool argument structs, plus static provider-capability declarations
//! (`#[derive(LlmPlugin)]` / `#[derive(TtsPlugin)]` / `#[derive(SttPlugin)]` /
//! `#[derive(VadPlugin)]`).
//!
//! - `#[derive(ToolSpec)]` — spec/profile construction from `#[tool(...)]`/`#[arg(...)]`
//! - `#[derive(ToolAction)]` — `ToolSpec` + an automatic `impl ToolAction`
//! - `#[tool_action(args = ...)]` — fills `name()`/`definition()`/`rag_profile()`
//!   forwarders on a hand-written `ToolAction` impl
//! - `#[derive(LlmPlugin)]` / `#[derive(TtsPlugin)]` / `#[derive(SttPlugin)]` /
//!   `#[derive(VadPlugin)]` — static spec constructors (`llm_spec()` /
//!   `tts_spec()` / `stt_spec()` / `vad_spec()`) and per-trait kind consts
//!   from a `#[provider(...)]` attribute
//!
//! See `docs/reference/tools/derive-macro.md` for the full attribute reference.

#![expect(
    clippy::option_if_let_else,
    reason = "nursery style; match/if-let clarity preferred in derive codegen"
)]

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{DeriveInput, parse_macro_input};

mod attr;
mod provider;

use attr::{ArgAttrs, ToolSpecAttrs, has_tool_skip};

/// Derive macro generating the static LLM capability declaration (an
/// inherent `llm_spec()` constructor and `LLM_PROVIDER_KIND` const) from a
/// `#[provider(...)]` attribute.
///
/// The `impl LlmPlugin` block stays hand-written: a one-line
/// `fn llm_capabilities(&self) -> Vec<LlmProviderSpec> { vec![Self::llm_spec()] }`
/// alongside the async handlers (`create_chat_stream`, `chat_completion`).
#[proc_macro_derive(LlmPlugin, attributes(provider))]
pub fn derive_llm_plugin(input: TokenStream) -> TokenStream {
    provider::expand_plugin(input, provider::ProviderKind::Llm)
}

/// Derive macro generating the static TTS capability declaration (an
/// inherent `tts_spec()` constructor and `TTS_PROVIDER_KIND` const) from a
/// `#[provider(...)]` attribute.
///
/// The `impl TtsPlugin` block stays hand-written: a one-line
/// `fn tts_capabilities(&self) -> Vec<TtsProviderSpec> { vec![Self::tts_spec()] }`
/// alongside the async `synthesize` handler.
///
/// Capability declarations (`provides` / `requires` in the attribute) are
/// emitted by the `LlmPlugin` derive only; on a Tts-only (or Stt-only)
/// derive they are validated but not generated — pair the derive with
/// `LlmPlugin` on the same struct to declare plugin-wide capabilities.
#[proc_macro_derive(TtsPlugin, attributes(provider))]
pub fn derive_tts_plugin(input: TokenStream) -> TokenStream {
    provider::expand_plugin(input, provider::ProviderKind::Tts)
}

/// Derive macro generating the static STT capability declaration (an
/// inherent `stt_spec()` constructor and `STT_PROVIDER_KIND` const) from a
/// `#[provider(...)]` attribute.
///
/// The `impl SttPlugin` block stays hand-written: a one-line
/// `fn stt_capabilities(&self) -> Vec<SttProviderSpec> { vec![Self::stt_spec()] }`
/// alongside the async `transcribe` handler.
///
/// Capability declarations (`provides` / `requires` in the attribute) are
/// emitted by the `LlmPlugin` derive only; on an Stt-only derive they are
/// validated but not generated — pair the derive with `LlmPlugin` on the
/// same struct to declare plugin-wide capabilities.
#[proc_macro_derive(SttPlugin, attributes(provider))]
pub fn derive_stt_plugin(input: TokenStream) -> TokenStream {
    provider::expand_plugin(input, provider::ProviderKind::Stt)
}

/// Derive macro generating the static VAD capability declaration (an
/// inherent `vad_spec()` constructor and `VAD_PROVIDER_KIND` const) from a
/// `#[provider(...)]` attribute.
///
/// The `impl VadPlugin` block stays hand-written: a one-line
/// `fn vad_capabilities(&self) -> Vec<VadProviderSpec> { vec![Self::vad_spec()] }`
/// alongside the async `process_chunk` handler.
///
/// The attribute must set `frame_size` (PCM samples per `ProcessVadChunk`
/// call; e.g. `frame_size = 512` for Silero VAD). Capability declarations
/// (`provides` / `requires` in the attribute) are emitted by the `LlmPlugin`
/// derive only; on a Vad-only derive they are validated but not generated —
/// pair the derive with `LlmPlugin` on the same struct to declare
/// plugin-wide capabilities.
#[proc_macro_derive(VadPlugin, attributes(provider))]
pub fn derive_vad_plugin(input: TokenStream) -> TokenStream {
    provider::expand_plugin(input, provider::ProviderKind::Vad)
}

/// Derive macro that generates `ToolSpec::spec()` on the args struct.
///
/// Reads from `#[tool(...)]` container attributes, `#[arg(...)]` per-field
/// attributes, and `///` doc comments. Also requires `JsonSchema` and
/// `Deserialize` to be derived by the user so that JSON Schema generation
/// works.
///
/// The generated `spec()` method:
///
/// 1. Builds a base JSON schema from `schemars` over `Self`.
/// 2. Forces `additionalProperties: false` on the root object so the LLM
///    cannot invent fields.
/// 3. Applies per-field `#[arg(...)]` overrides (hide, `enum_values`,
///    default, min/max, alias-in-description).
///
/// The name (`TOOL_NAME`), display name, and summary are emitted as
/// `pub const`s so that the dispatch trait, the spec's `name` field, and
/// any test all share the same string literal.
#[proc_macro_derive(ToolSpec, attributes(tool, arg, tool_meta))]
pub fn derive_tool_spec(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    expand_tool_spec(&ast)
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

/// Derive macro that combines `#[derive(ToolSpec)]` with an automatic
/// `impl ToolAction` block.
///
/// The struct's fields are the JSON args. Fields marked with
/// `#[tool(skip)]` (or `#[arg(skip)]`) are stateful — they are hidden
/// from the schema, not deserialized from JSON, and copied from `self`
/// in the generated `execute()` method. **All `#[tool(skip)]` fields
/// must implement `Clone`** because the generated code calls
/// `.clone()` to copy them from `self` into the deserialized args.
///
/// The user must define `async fn run(&self) -> Result<String, ToolError>`
/// in a separate `impl` block.
///
/// # Example
///
/// ```ignore
/// use ene_plugin::prelude::*;
///
/// #[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
/// #[tool(namespace = "app", name = "press_key",
///        summary = "Press a key.", category = "App",
///        keywords_primary = "keyboard, press, key")]
/// pub struct PressKeyAction {
///     /// Key name.
///     key: String,
/// }
///
/// impl PressKeyAction {
///     async fn run(&self) -> Result<String, ToolError> {
///         Ok(format!("Pressed {}", self.key))
///     }
/// }
/// ```
///
/// For stateful actions (e.g. with a sandbox):
///
/// ```ignore
/// #[derive(ToolAction)]
/// #[tool(namespace = "filesystem", name = "write", ...)]
/// pub struct FsWriteAction {
///     #[tool(skip)]
///     #[serde(skip, default = "default_sandbox")]
///     sandbox: Arc<RwLock<Option<Arc<Sandbox>>>>,
///     file_path: String,
///     content: String,
/// }
/// ```
#[proc_macro_derive(ToolAction, attributes(tool, arg, tool_meta))]
pub fn derive_tool_action(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match expand_tool_action_derive(&ast) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_tool_action_derive(ast: &DeriveInput) -> syn::Result<TokenStream2> {
    let spec_output = expand_tool_spec(ast)?;

    let ident = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let syn::Data::Struct(data) = &ast.data else {
        return Err(syn::Error::new_spanned(
            ast,
            "ToolAction can only be derived on structs",
        ));
    };
    let fields = match &data.fields {
        syn::Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(
                ast,
                "ToolAction requires a struct with named fields",
            ));
        }
    };

    let skip_copies: Vec<TokenStream2> = fields
        .iter()
        .filter(|f| {
            let arg = ArgAttrs::from_field(f).unwrap_or_default();
            arg.skip || has_tool_skip(f)
        })
        .filter_map(|f| f.ident.as_ref())
        .map(|field_ident| {
            quote! { args.#field_ident = self.#field_ident.clone(); }
        })
        .collect();

    let output = quote! {
        #spec_output

        #[::async_trait::async_trait]
        impl #impl_generics ::ene_plugin::ToolAction for #ident #ty_generics #where_clause {
            fn name(&self) -> &'static str {
                <Self as ::ene_plugin::ToolSpecArgs>::TOOL_NAME
            }

            fn definition(&self) -> ::ene_plugin_proto::ToolSpec {
                <Self as ::ene_plugin::ToolSpecArgs>::spec()
            }

            fn rag_profile(&self) -> ::ene_plugin_proto::ToolRagProfile {
                <Self as ::ene_plugin::ToolSpecArgs>::rag_profile()
            }

            async fn execute(&self, arguments: &str) -> ::std::result::Result<::std::string::String, ::ene_plugin_proto::ToolError> {
                let mut args: Self = ::serde_json::from_str(arguments).map_err(|e| {
                    ::ene_plugin_proto::ToolError::InvalidArguments {
                        message: ::std::format!(
                            "Invalid arguments for {}: {e}",
                            <Self as ::ene_plugin::ToolSpecArgs>::TOOL_NAME
                        ),
                    }
                })?;
                #(#skip_copies)*
                args.run().await
            }
        }
    };

    Ok(output)
}

/// Attribute macro that fills in the `name()` and `definition()`
/// forwarders on a `ToolAction` impl block, sourcing both from the
/// args struct's `TOOL_NAME` const and `spec()` method.
///
/// Usage:
///
/// ```ignore
/// #[tool_action(args = MyArgs)]
/// #[async_trait]
/// impl ToolAction for MyAction {
///     async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
///         let args: MyArgs = serde_json::from_str(arguments)?;
///         // body
///     }
/// }
/// ```
///
/// The drift-prone forwarders (`name`, `definition`, `rag_profile`) are
/// auto-generated; the `execute` shim is written by the user.
#[proc_macro_attribute]
pub fn tool_action(attr: TokenStream, input: TokenStream) -> TokenStream {
    let args_ty: ToolActionAttr = parse_macro_input!(attr as ToolActionAttr);
    let mut item = parse_macro_input!(input as syn::ItemImpl);
    expand_tool_action(&mut item, &args_ty.0);
    quote! { #item }.into()
}

/// Parses `args = MyArgs` from the attribute tokens.
struct ToolActionAttr(syn::Type);

impl Parse for ToolActionAttr {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ident: syn::Ident = input.parse()?;
        if ident != "args" {
            return Err(syn::Error::new_spanned(ident, "expected `args = MyArgs`"));
        }
        let _eq: syn::Token![=] = input.parse()?;
        let ty: syn::Type = input.parse()?;
        if !input.is_empty() {
            return Err(syn::Error::new_spanned(
                ty,
                "unexpected trailing tokens after `args = MyArgs`",
            ));
        }
        Ok(Self(ty))
    }
}

fn expand_tool_action(item: &mut syn::ItemImpl, args_ty: &syn::Type) {
    if let Some((trait_path, _)) = &item.trait_ {
        let is_tool_action = trait_path
            .segments
            .last()
            .is_some_and(|s| s.ident == "ToolAction");
        if !is_tool_action {
            return;
        }
    }
    let name_fn: syn::ImplItem = syn::parse_quote! {
        fn name(&self) -> &'static str {
            <#args_ty as ::ene_plugin::ToolSpecArgs>::TOOL_NAME
        }
    };
    let def_fn: syn::ImplItem = syn::parse_quote! {
        fn definition(&self) -> ::ene_plugin_proto::ToolSpec {
            <#args_ty as ::ene_plugin::ToolSpecArgs>::spec()
        }
    };
    let rag_fn: syn::ImplItem = syn::parse_quote! {
        fn rag_profile(&self) -> ::ene_plugin_proto::ToolRagProfile {
            <#args_ty as ::ene_plugin::ToolSpecArgs>::rag_profile()
        }
    };
    item.items.push(name_fn);
    item.items.push(def_fn);
    item.items.push(rag_fn);
}

fn expand_tool_spec(ast: &DeriveInput) -> syn::Result<TokenStream2> {
    let struct_attrs = ToolSpecAttrs::from_derive_input(ast)
        .map_err(|e| syn::Error::new_spanned(ast, e.to_string()))?;

    let ident = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let syn::Data::Struct(data) = &ast.data else {
        return Err(syn::Error::new_spanned(
            ast,
            "ToolSpec can only be derived on structs",
        ));
    };
    let fields = match &data.fields {
        syn::Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(
                ast,
                "ToolSpec requires a struct with named fields",
            ));
        }
    };

    let tool_name_str = struct_attrs.full_name();
    let display_name = struct_attrs.display_name_value();
    let summary = struct_attrs
        .summary_value()
        .map_err(|e| syn::Error::new_spanned(ast, e.to_string()))?;
    let description = struct_attrs.description_value();
    let category = struct_attrs.category_path();
    let side_effects = struct_attrs.side_effects_path();
    let side_effects_spec = struct_attrs.side_effects_spec();
    let background_capable = struct_attrs.background_capable;
    let keywords_primary = struct_attrs.keywords_list("primary");
    let keywords_secondary = struct_attrs.keywords_list("secondary");
    let keywords_domain = struct_attrs.keywords_list("domain");
    let keywords_negative = struct_attrs.keywords_list("negative");
    let caveats = struct_attrs.string_list("caveats");
    let preconditions = struct_attrs.string_list("preconditions");
    let related = struct_attrs.related_list();
    let version = struct_attrs
        .version_tokens()
        .map_err(|e| syn::Error::new_spanned(ast, e.to_string()))?;
    let examples = struct_attrs.examples_value();
    let args_const_ident = struct_attrs.args_const_ident(ident);

    let field_instrs = collect_field_instructions(fields)?;

    let output = quote! {
        impl #impl_generics #ident #ty_generics #where_clause {
            /// Canonical tool name (e.g. "filesystem.read"). The same const
            /// is also used as the `ToolSpec::name` field, so dispatch and
            /// the LLM-visible identifier are guaranteed to agree.
            pub const TOOL_NAME: &'static str = #tool_name_str;

            /// Short human-friendly display name.
            pub const DISPLAY_NAME: &'static str = #display_name;

            /// One-line summary used as the primary embedding field.
            pub const SUMMARY: &'static str = #summary;

            /// Construct a `ToolSpec` for this args type.
            ///
            /// Emits the LLM-facing fields (`name`, `description`,
            /// `parameters`) plus the host-execution metadata
            /// `background_capable` and `side_effects` (the latter drives the
            /// parallel tool-call policy). Rich `#[tool(...)]` metadata
            /// is emitted by [`Self::rag_profile`] for Tool RAG.
            pub fn spec() -> ::ene_plugin_proto::ToolSpec {
                use ::ene_plugin_proto::{ToolSpec, ToolName};
                use ::schemars::JsonSchema as _;
                use ::serde_json::json;

                let mut schema: ::serde_json::Value = {
                    let g = ::schemars::SchemaGenerator::default();
                    let s = g.into_root_schema_for::<Self>();
                    ::serde_json::to_value(s).unwrap_or_else(|_| json!({"type": "object"}))
                };

                if let Some(obj) = schema.as_object_mut() {
                    obj.insert("additionalProperties".to_string(), ::serde_json::Value::Bool(false));
                }

                #(#field_instrs)*

                let description = {
                    let from_attr = #description;
                    if from_attr.is_empty() {
                        #summary.to_string()
                    } else {
                        from_attr.to_string()
                    }
                };

                ToolSpec {
                    name: ToolName::new(#tool_name_str),
                    description,
                    parameters: schema,
                    background_capable: #background_capable,
                    side_effects: #side_effects_spec,
                }
            }

            /// Construct the host/RAG metadata profile for this args type.
            pub fn rag_profile() -> ::ene_plugin_proto::ToolRagProfile {
                use ::ene_plugin_proto::{
                    KeywordSet, ToolName, ToolRagProfile, ToolVersion,
                };

                let description = {
                    let from_attr = #description;
                    if from_attr.is_empty() {
                        #summary.to_string()
                    } else {
                        from_attr.to_string()
                    }
                };

                ToolRagProfile {
                    name: ToolName::new(#tool_name_str),
                    display_name: #display_name.to_string(),
                    summary: #summary.to_string(),
                    description,
                    category: #category,
                    keywords: KeywordSet {
                        primary: vec![#(#keywords_primary.into()),*],
                        secondary: vec![#(#keywords_secondary.into()),*],
                        domain: vec![#(#keywords_domain.into()),*],
                        negative: vec![#(#keywords_negative.into()),*],
                    },
                    examples: #examples,
                    caveats: vec![#(#caveats.into()),*],
                    preconditions: vec![#(#preconditions.into()),*],
                    side_effects: #side_effects,
                    related: vec![#(ToolName::new(#related)),*],
                    version: #version,
                }
            }
        }

        impl #impl_generics ::ene_plugin::ToolSpecArgs for #ident #ty_generics #where_clause {
            const TOOL_NAME: &'static str = #tool_name_str;
            fn spec() -> ::ene_plugin_proto::ToolSpec {
                <Self>::spec()
            }
            fn rag_profile() -> ::ene_plugin_proto::ToolRagProfile {
                <Self>::rag_profile()
            }
        }

    #[expect(dead_code, reason = "generated const anchors ToolSpec expansion")]
    const #args_const_ident: () = ();
    };

    Ok(output)
}

/// Per-field post-processing data extracted from `#[arg(...)]` and
/// `#[serde(...)]` attributes.
struct FieldInstr {
    /// JSON property key (honoring `#[serde(rename = "...")]`).
    prop_key: String,
    /// `#[serde(alias = "...")]` values, in declaration order.
    aliases: Vec<String>,
    /// `#[arg(description = "...")]`, if any.
    description: Option<String>,
    /// `#[arg(internal)]` or `#[arg(hidden)]`.
    hidden: bool,
    /// `#[arg(enum_values = "a,b")]`, if any.
    enum_values: Vec<String>,
    /// `#[arg(default = "...")]`, if any.
    ///
    /// # Type Inference Ambiguity
    ///
    /// The default value is provided as a string in the attribute, and the macro
    /// attempts to infer its underlying JSON type. If the string parses as an
    /// integer or float, it is emitted as a JSON number. If it is exactly "true"
    /// or "false", it is emitted as a JSON boolean. "null" becomes JSON null.
    ///
    /// This causes ambiguity when a field is meant to be a string but its default
    /// value looks like a number (e.g. `#[arg(default = "1.0")]` for a version
    /// string). To force the value to be emitted as a JSON string, the author
    /// must include escaped quotes: `#[arg(default = "\"1.0\"")]`.
    default: Option<String>,
    /// Numeric / string / array length constraints.
    minimum: Option<i64>,
    maximum: Option<i64>,
    min_length: Option<usize>,
    max_length: Option<usize>,
    min_items: Option<usize>,
    max_items: Option<usize>,
}

fn collect_field_instructions(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::token::Comma>,
) -> syn::Result<Vec<TokenStream2>> {
    let mut out = Vec::new();
    for f in fields {
        let arg = ArgAttrs::from_field(f).map_err(|e| syn::Error::new_spanned(f, e.to_string()))?;
        let field_name = f
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(f, "ToolSpec requires named fields"))?
            .to_string();

        let mut instr = FieldInstr {
            prop_key: field_name.clone(),
            aliases: Vec::new(),
            description: arg.description.clone(),
            hidden: arg.is_hidden() || has_tool_skip(f),
            enum_values: arg.enum_values.0.clone(),
            default: arg.default.clone(),
            minimum: arg.minimum,
            maximum: arg.maximum,
            min_length: arg.min_length,
            max_length: arg.max_length,
            min_items: arg.min_items,
            max_items: arg.max_items,
        };
        apply_serde_attrs(f, &mut instr);
        out.push(emit_field(&instr));
    }
    Ok(out)
}

/// Walk `#[serde(rename = "...", alias = "...")]` on a field and update
/// the `FieldInstr` in place.
fn apply_serde_attrs(f: &syn::Field, instr: &mut FieldInstr) {
    for attr in &f.attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        // Best-effort: an unparseable `#[serde(...)]` shape just leaves the
        // field's defaults untouched, so the parse error is intentionally
        // discarded (via `drop`, not `let _ =`, to satisfy
        // `clippy::let_underscore_must_use`).
        drop(attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                if let Ok(lit) = meta
                    .value()
                    .and_then(syn::parse::ParseBuffer::parse::<syn::LitStr>)
                {
                    instr.prop_key = lit.value();
                }
            } else if meta.path.is_ident("alias")
                && let Ok(lit) = meta
                    .value()
                    .and_then(syn::parse::ParseBuffer::parse::<syn::LitStr>)
            {
                instr.aliases.push(lit.value());
            }
            Ok(())
        }));
    }
}

fn emit_field(instr: &FieldInstr) -> TokenStream2 {
    let key = instr.prop_key.as_str();

    if instr.hidden {
        return quote! {
            {
                if let Some(p) = schema
                    .get_mut("properties")
                    .and_then(|p| p.as_object_mut())
                {
                    p.remove(#key);
                }
                if let Some(r) = schema
                    .get_mut("required")
                    .and_then(|r| r.as_array_mut())
                {
                    r.retain(|v| v.as_str() != Some(#key));
                }
            }
        };
    }

    let mut edits: Vec<TokenStream2> = Vec::new();

    if let Some(d) = &instr.description {
        edits.push(quote! {
            prop.entry("description".to_string())
                .or_insert(::serde_json::json!(#d));
        });
    }

    if !instr.enum_values.is_empty() {
        let vals: Vec<&str> = instr
            .enum_values
            .iter()
            .map(std::string::String::as_str)
            .collect();
        edits.push(quote! {
            prop.insert("enum".to_string(), ::serde_json::json!([#(#vals),*]));
        });
    }
    if let Some(default) = &instr.default {
        let parsed = default.trim();
        let value_expr: TokenStream2 = if let Ok(n) = parsed.parse::<i64>() {
            quote! { ::serde_json::json!(#n) }
        } else if let Ok(f) = parsed.parse::<f64>() {
            // Parse floats before the string fallback
            // so a #[arg(default = "1.5")] on an f64
            // produces a number 1.5 in the JSON
            // schema, not the string "1.5".
            quote! { ::serde_json::json!(#f) }
        } else if parsed == "true" {
            quote! { ::serde_json::json!(true) }
        } else if parsed == "false" {
            quote! { ::serde_json::json!(false) }
        } else if parsed == "null" {
            quote! { ::serde_json::json!(null) }
        } else {
            let lit = parsed.trim_matches('"');
            quote! { ::serde_json::json!(#lit) }
        };
        edits.push(quote! {
            prop.insert("default".to_string(), #value_expr);
        });
    }
    if let Some(min) = instr.minimum {
        edits.push(quote! { prop.insert("minimum".to_string(), ::serde_json::json!(#min)); });
    }
    if let Some(max) = instr.maximum {
        edits.push(quote! { prop.insert("maximum".to_string(), ::serde_json::json!(#max)); });
    }
    if let Some(n) = instr.min_length {
        edits.push(quote! { prop.insert("minLength".to_string(), ::serde_json::json!(#n)); });
    }
    if let Some(n) = instr.max_length {
        edits.push(quote! { prop.insert("maxLength".to_string(), ::serde_json::json!(#n)); });
    }
    if let Some(n) = instr.min_items {
        edits.push(quote! { prop.insert("minItems".to_string(), ::serde_json::json!(#n)); });
    }
    if let Some(n) = instr.max_items {
        edits.push(quote! { prop.insert("maxItems".to_string(), ::serde_json::json!(#n)); });
    }

    let alias_block = if instr.aliases.is_empty() {
        quote! {}
    } else {
        let lits: Vec<String> = instr.aliases.iter().map(|a| format!("`{a}`")).collect();
        let concat = lits.join(", ");
        let append = format!(" Aliases: {concat}.");
        quote! {
            if let Some(d) = prop.get("description").and_then(|d| d.as_str()) {
                let mut s = String::from(d);
                s.push_str(#append);
                prop.insert("description".to_string(), ::serde_json::json!(s));
            } else {
                let mut s = String::new();
                s.push_str(#append.trim_start());
                prop.insert("description".to_string(), ::serde_json::json!(s));
            }
        }
    };

    quote! {
        {
            if let Some(p) = schema
                .get_mut("properties")
                .and_then(|p| p.as_object_mut())
            {
                if let Some(prop_val) = p.get_mut(#key) {
                    if let Some(prop) = prop_val.as_object_mut() {
                        #(#edits)*
                        #alias_block
                    }
                }
            }
        }
    }
}
