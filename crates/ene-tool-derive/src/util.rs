//! Small helpers used by the derive.

/// Concatenate the `#[doc = "..."]` attributes of an item into a single
/// `String`. Returns an empty string when no doc comment is present.
pub fn doc_comment_text(attrs: &[syn::Attribute]) -> String {
    let mut out = String::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta
            && let syn::Expr::Lit(lit) = &nv.value
            && let syn::Lit::Str(s) = &lit.lit
        {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&s.value());
        }
    }
    out
}
