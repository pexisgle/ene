//! Provider capability derive expansion: `#[derive(LlmPlugin)]`,
//! `#[derive(TtsPlugin)]`, `#[derive(SttPlugin)]`, and
//! `#[derive(VadPlugin)]` (entry points live in `lib.rs` — proc-macro
//! functions must sit at the crate root).
//!
//! All four derives read a single shared `#[provider(...)]` container
//! attribute and expand to inherent items only — a per-trait static spec
//! constructor (`llm_spec()` / `tts_spec()` / `stt_spec()`) and a per-trait
//! kind const (`LLM_PROVIDER_KIND` / `TTS_PROVIDER_KIND` /
//! `STT_PROVIDER_KIND` / `VAD_PROVIDER_KIND`).
//!
//! The `impl LlmPlugin` / `TtsPlugin` / `SttPlugin` block is written by the
//! user (a one-line `*_capabilities()` returning `vec![Self::<trait>_spec()]`
//! plus their hand-written async handlers) — a derive cannot generate that
//! impl because Rust rejects a second `impl` block for the same trait on the
//! same type (E0119), which would be required to attach async handlers.
//! Async handlers and `config_schema` are therefore never generated.
//!
//! Consts and spec constructors are named per trait (rather than a shared
//! `PROVIDER_KIND` / `spec()`) because rustc strips `#[derive(...)]` from a
//! derive macro's input, so two derives on one struct (a compound provider)
//! cannot coordinate ownership of a same-named inherent item.
//!
//! `EmbedPlugin` is deliberately out of scope: it has no static capability
//! declaration to generate (`embed_batch` is the entire trait).
//!
//! `#[provider(provides = "...", requires = "...")]` declares plugin-wide
//! capabilities; the derive emits the `provides()` / `requires()` methods
//! from the `LlmPlugin` expansion only (see `expand_plugin_derive`). On a
//! Tts-only or Stt-only derive the literals are still validated against the
//! capability grammar but no method is generated — pair the derive with
//! `LlmPlugin` to declare capabilities.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{DeriveInput, LitInt, parse_macro_input};

use crate::attr::{parse_flag, path_ident_str};

use ene_plugin_proto::{CapabilityRef, CapabilityRequirement};

/// Which provider trait a derive expands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderKind {
    Llm,
    Tts,
    Stt,
    Vad,
}

impl ProviderKind {
    /// Inherent const name generated on the plugin struct.
    fn const_name(self) -> &'static str {
        match self {
            Self::Llm => "LLM_PROVIDER_KIND",
            Self::Tts => "TTS_PROVIDER_KIND",
            Self::Stt => "STT_PROVIDER_KIND",
            Self::Vad => "VAD_PROVIDER_KIND",
        }
    }

    /// Inherent static spec constructor generated on the plugin struct.
    fn spec_method_name(self) -> &'static str {
        match self {
            Self::Llm => "llm_spec",
            Self::Tts => "tts_spec",
            Self::Stt => "stt_spec",
            Self::Vad => "vad_spec",
        }
    }
}

/// Parsed contents of the single `#[provider(...)]` attribute.
///
/// The parser accepts the union of all keys so one attribute can feed a
/// compound provider (`#[derive(LlmPlugin, TtsPlugin)]`); `kind` is shared by
/// every provider derive on the struct.
#[derive(Debug, Default)]
struct ProviderAttrs {
    kind: Option<String>,
    models: Vec<String>,
    voices: Vec<String>,
    formats: Vec<String>,
    streaming: bool,
    vision: bool,
    max_in_flight: Option<u32>,
    queue_depth: Option<u32>,
    context_window: Option<u32>,
    frame_size: Option<u32>,
    provides: Vec<String>,
    requires: Vec<String>,
}

impl ProviderAttrs {
    fn from_derive_input(ast: &DeriveInput) -> syn::Result<Self> {
        let mut attrs = Self::default();
        let mut found = false;

        for attr in &ast.attrs {
            if !attr.path().is_ident("provider") {
                continue;
            }
            if found {
                return Err(syn::Error::new_spanned(
                    attr,
                    "only one `#[provider(...)]` attribute is supported; a compound \
                     provider shares a single `kind` across its derives",
                ));
            }
            found = true;
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("kind") {
                    let s: syn::LitStr = meta.value()?.parse()?;
                    attrs.kind = Some(s.value());
                } else if meta.path.is_ident("models") {
                    let s: syn::LitStr = meta.value()?.parse()?;
                    attrs.models = split_list(&s.value());
                } else if meta.path.is_ident("voices") {
                    let s: syn::LitStr = meta.value()?.parse()?;
                    attrs.voices = split_list(&s.value());
                } else if meta.path.is_ident("formats") {
                    let s: syn::LitStr = meta.value()?.parse()?;
                    attrs.formats = split_list(&s.value());
                } else if meta.path.is_ident("streaming") {
                    attrs.streaming = parse_flag(&meta)?;
                } else if meta.path.is_ident("vision") {
                    attrs.vision = parse_flag(&meta)?;
                } else if meta.path.is_ident("concurrency") {
                    attrs.max_in_flight = Some(parse_u32(&meta)?);
                } else if meta.path.is_ident("queue_depth") {
                    attrs.queue_depth = Some(parse_u32(&meta)?);
                } else if meta.path.is_ident("context_window") {
                    attrs.context_window = Some(parse_u32(&meta)?);
                } else if meta.path.is_ident("frame_size") {
                    attrs.frame_size = Some(parse_u32(&meta)?);
                } else if meta.path.is_ident("provides") {
                    let s: syn::LitStr = meta.value()?.parse()?;
                    attrs.provides = validate_capability_items(&s, "provides", |item| {
                        CapabilityRef::parse(item).map(|_| ())
                    })?;
                } else if meta.path.is_ident("requires") {
                    let s: syn::LitStr = meta.value()?.parse()?;
                    attrs.requires = validate_capability_items(&s, "requires", |item| {
                        CapabilityRequirement::parse(item).map(|_| ())
                    })?;
                } else {
                    let name = path_ident_str(&meta.path);
                    return Err(syn::Error::new_spanned(
                        meta.path,
                        format!("unknown provider attribute: `{name}`"),
                    ));
                }
                Ok(())
            })?;
        }

        if !found {
            return Err(syn::Error::new_spanned(
                ast,
                "missing `#[provider(kind = \"...\")]` attribute",
            ));
        }
        if attrs.kind.as_deref().is_none_or(str::is_empty) {
            return Err(syn::Error::new_spanned(
                ast,
                "`#[provider(kind = \"...\")]` is required and must not be empty",
            ));
        }
        Ok(attrs)
    }
}

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

/// Validates a comma-separated capability declaration list against the wire
/// grammar, returning the split items.
///
/// The generated `provides()` / `requires()` methods re-parse these literals
/// at runtime; validating here turns a typo into a compile error instead of
/// a startup panic in the plugin process.
fn validate_capability_items<E: std::fmt::Display>(
    lit: &syn::LitStr,
    attribute: &str,
    parse: impl Fn(&str) -> Result<(), E>,
) -> syn::Result<Vec<String>> {
    let items = split_list(&lit.value());
    for item in &items {
        if let Err(err) = parse(item) {
            return Err(syn::Error::new_spanned(
                lit,
                format!("invalid `{attribute}` capability literal `{item}`: {err}"),
            ));
        }
    }
    Ok(items)
}

fn parse_u32(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<u32> {
    let lit: LitInt = meta.value()?.parse()?;
    lit.base10_parse()
}

/// Inline `ConcurrencyHint` construction honoring the serial default for any
/// field the author did not set explicitly.
fn concurrency_expr(max_in_flight: Option<u32>, queue_depth: Option<u32>) -> TokenStream2 {
    match (max_in_flight, queue_depth) {
        (None, None) => quote! { ::ene_plugin::ConcurrencyHint::default() },
        (max, queue) => {
            let max_in_flight = if let Some(n) = max {
                quote! { #n }
            } else {
                quote! { ::ene_plugin::ConcurrencyHint::default().max_in_flight }
            };
            let queue_depth = if let Some(n) = queue {
                quote! { #n }
            } else {
                quote! { ::ene_plugin::ConcurrencyHint::default().queue_depth }
            };
            quote! {
                ::ene_plugin::ConcurrencyHint {
                    max_in_flight: #max_in_flight,
                    queue_depth: #queue_depth,
                }
            }
        }
    }
}

pub(crate) fn expand_plugin(input: TokenStream, kind: ProviderKind) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match expand_plugin_derive(&ast, kind) {
        Ok(ts) => ts.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_plugin_derive(ast: &DeriveInput, kind: ProviderKind) -> syn::Result<TokenStream2> {
    let attrs = ProviderAttrs::from_derive_input(ast)?;
    let kind_str = attrs.kind.as_deref().unwrap_or_default();
    let const_name = syn::Ident::new(kind.const_name(), proc_macro2::Span::call_site());
    let spec_method = syn::Ident::new(kind.spec_method_name(), proc_macro2::Span::call_site());
    let ident = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();

    let spec_fn = match kind {
        ProviderKind::Llm => {
            let models = attrs.models.iter().map(String::as_str);
            let streaming = attrs.streaming;
            let vision = attrs.vision;
            let concurrency = concurrency_expr(attrs.max_in_flight, attrs.queue_depth);
            let context_window = if let Some(n) = attrs.context_window {
                quote! { ::std::option::Option::Some(#n) }
            } else {
                quote! { ::std::option::Option::None }
            };
            quote! {
                pub fn #spec_method() -> ::ene_plugin::LlmProviderSpec {
                    ::ene_plugin::LlmProviderSpec {
                        kind: #kind_str.to_string(),
                        supported_models: ::std::vec![#(#models.to_string()),*],
                        supports_streaming: #streaming,
                        supports_vision: #vision,
                        concurrency: #concurrency,
                        context_window: #context_window,
                    }
                }
            }
        }
        ProviderKind::Tts => {
            let voices = attrs.voices.iter().map(String::as_str);
            let formats = attrs.formats.iter().map(String::as_str);
            let concurrency = concurrency_expr(attrs.max_in_flight, attrs.queue_depth);
            quote! {
                pub fn #spec_method() -> ::ene_plugin::TtsProviderSpec {
                    ::ene_plugin::TtsProviderSpec {
                        kind: #kind_str.to_string(),
                        voices: ::std::vec![#(#voices.to_string()),*],
                        formats: ::std::vec![#(#formats.to_string()),*],
                        concurrency: #concurrency,
                    }
                }
            }
        }
        ProviderKind::Stt => {
            let models = attrs.models.iter().map(String::as_str);
            let formats = attrs.formats.iter().map(String::as_str);
            let concurrency = concurrency_expr(attrs.max_in_flight, attrs.queue_depth);
            quote! {
                pub fn #spec_method() -> ::ene_plugin::SttProviderSpec {
                    ::ene_plugin::SttProviderSpec {
                        kind: #kind_str.to_string(),
                        models: ::std::vec![#(#models.to_string()),*],
                        formats: ::std::vec![#(#formats.to_string()),*],
                        concurrency: #concurrency,
                    }
                }
            }
        }
        ProviderKind::Vad => {
            let frame_size = attrs.frame_size.ok_or_else(|| {
                syn::Error::new_spanned(
                    ast,
                    "VadPlugin requires `frame_size = N` in #[provider(...)] \
                     (PCM samples per ProcessVadChunk call)",
                )
            })?;
            let concurrency = concurrency_expr(attrs.max_in_flight, attrs.queue_depth);
            quote! {
                pub fn #spec_method() -> ::ene_plugin::VadProviderSpec {
                    ::ene_plugin::VadProviderSpec {
                        kind: #kind_str.to_string(),
                        frame_size: #frame_size,
                        concurrency: #concurrency,
                    }
                }
            }
        }
    };

    let const_doc = format!(
        "Provider kind identifier for this trait's capabilities; e.g. the \
         hand-written kind guards in async handlers compare against \
         `Self::{const_name}`."
    );

    // Capability declarations are plugin-wide, so they are emitted from the
    // `derive(LlmPlugin)` expansion only. A compound provider
    // (`derive(LlmPlugin, TtsPlugin)`) shares one `#[provider]` attribute:
    // rustc hands each derive its own reduced copy of `#[derive(...)]`, so a
    // Tts/Stt expansion cannot tell whether `LlmPlugin` is also derived and
    // must not emit a second, colliding definition.
    let generate_capability_methods = matches!(kind, ProviderKind::Llm);
    let capability_methods = if generate_capability_methods {
        let provides = attrs.provides.iter().map(String::as_str);
        let requires = attrs.requires.iter().map(String::as_str);
        let provides_fn = if attrs.provides.is_empty() {
            quote! {}
        } else {
            quote! {
                /// Capabilities this plugin provides to other plugins,
                /// declared by the `#[provider(provides = "...")]` attribute.
                #[expect(
                    clippy::unwrap_used,
                    reason = "provider attribute capability strings are \
                              validated against the capability grammar at \
                              macro expansion, so the runtime parse cannot fail"
                )]
                pub fn provides() -> ::std::vec::Vec<::ene_plugin::CapabilityRef> {
                    ::std::vec![
                        #(::ene_plugin::CapabilityRef::parse(#provides).unwrap()),*
                    ]
                }
            }
        };
        let requires_fn = if attrs.requires.is_empty() {
            quote! {}
        } else {
            quote! {
                /// Capabilities this plugin requires from other plugins,
                /// declared by the `#[provider(requires = "...")]` attribute.
                #[expect(
                    clippy::unwrap_used,
                    reason = "provider attribute capability strings are \
                              validated against the capability grammar at \
                              macro expansion, so the runtime parse cannot fail"
                )]
                pub fn requires() -> ::std::vec::Vec<::ene_plugin::CapabilityRequirement> {
                    ::std::vec![
                        #(::ene_plugin::CapabilityRequirement::parse(#requires).unwrap()),*
                    ]
                }
            }
        };
        quote! { #provides_fn #requires_fn }
    } else {
        quote! {}
    };

    Ok(quote! {
        impl #impl_generics #ident #ty_generics #where_clause {
            #[doc = #const_doc]
            pub const #const_name: &'static str = #kind_str;

            /// Static capability declaration backing the hand-written
            /// `*_capabilities()` trait method (which returns
            /// `vec![Self::#spec_method()]`).
            #spec_fn
            #capability_methods
        }
    })
}
