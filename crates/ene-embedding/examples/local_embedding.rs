//! Local embedding example using ene-embedding.
//!
//! Demonstrates loading a GGUF-quantized model from disk and computing
//! embeddings with cosine similarity comparison.
//!
//! Requires a GGUF model file in the `models/` directory.
//! Download from `HuggingFace`:
//!   huggingface-cli download jinaai/jina-embeddings-v5-text-small
//!
//! Requires a multi-thread tokio runtime. The GGUF forward
//! pass uses `tokio::task::block_in_place`, which panics on
//! a `current_thread` runtime (or outside any runtime). The
//! `#[tokio::main]` macro below uses the default
//! multi-thread flavor, so this example is correct as
//! written; consumers porting the example to their own
//! `Runtime::new()` must pass an explicit
//! `Builder::new_multi_thread()`.

use ene_embedding::{GgufEmbeddingProvider, resolve_gguf_paths};
use ene_provider::EmbeddingProvider;
use ene_provider::cosine_similarity;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model_name = "jina-embeddings-v5-text-small";
    let quantization = "F16";
    let model_dir = std::path::PathBuf::from("./models");

    // Resolve GGUF and tokenizer file paths
    let (gguf_path, tokenizer_path) = resolve_gguf_paths(model_name, quantization, model_dir)?;

    println!("Loading model: {model_name}");
    println!("GGUF path: {}", gguf_path.display());
    println!("Tokenizer path: {}", tokenizer_path.display());

    // Load the model (GPU-free, runs on CPU via Candle)
    let max_length = 8192;
    let provider = GgufEmbeddingProvider::load(
        model_name,
        gguf_path.to_str().unwrap_or(""),
        tokenizer_path.to_str().unwrap_or(""),
        max_length,
        quantization,
    )?;

    println!("Model dimensions: {}", provider.dimensions());
    println!("Model name: {}", provider.model_name());

    // Compute embeddings for three sentences. We
    // reuse the same provider and the same tokio
    // runtime (the one #[tokio::main] installed)
    // across all three calls; the previous form
    // constructed a fresh `Runtime::new()` per
    // embed_query, which is wasteful and would
    // panic on a `block_in_place` call from a
    // current_thread flavor.
    let text1 = "The cat sat on the mat.";
    let text2 = "A feline rested on a rug.";
    let text3 = "The stock market crashed today.";

    let emb1 = provider.embed_query(text1).await?;
    let emb2 = provider.embed_query(text2).await?;
    let emb3 = provider.embed_query(text3).await?;

    let similarity = cosine_similarity(&emb1, &emb2);
    let similarity_unrelated = cosine_similarity(&emb1, &emb3);

    println!("\nText 1: \"{text1}\"");
    println!("Text 2: \"{text2}\"");
    println!("Cosine similarity: {similarity:.4}");
    println!("\nText 3: \"{text3}\"");
    println!("Cosine similarity (cat vs stocks): {similarity_unrelated:.4}");

    assert!(
        similarity > similarity_unrelated,
        "Related texts should be more similar than unrelated ones"
    );

    println!("\nEmbeddings work correctly!");

    Ok(())
}
