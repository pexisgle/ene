//! Local GGUF embedding via llama-cpp-4.

mod error;
mod model;
mod provider;

pub use error::EneEmbeddingError;
pub use provider::GgufEmbeddingProvider;
