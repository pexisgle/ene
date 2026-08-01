//! Hybrid memory search scoring — re-exported from `ene-rag`.
//!
//! The scoring policy (weighted hybrid scoring, recency decay, lexical overlap)
//! lives in `ene-rag` (the RAG policy layer). Only the items store code reaches
//! through a `crate::search::*` path are re-exported here, at the narrowest
//! visibility that keeps those paths working:
//!
//! - `document_lexical_similarity` stays `pub` — it is genuinely public
//!   (re-exported at the crate root).
//! - `tokenize` stays `pub(crate)` — used internally by the store.
//!
//! Everything else is consumed directly as `ene_rag::*`, so it is not
//! re-exported here (an unused `pub(crate) use` would trip `unused_imports`).

pub use ene_rag::document_lexical_similarity;
pub(crate) use ene_rag::tokenize;
