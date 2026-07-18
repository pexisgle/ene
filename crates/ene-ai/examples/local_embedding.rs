//! Local embedding example using ene-ai (llama-cpp-2).
//!
//! Demonstrates loading a GGUF-quantized model from disk and computing
//! embeddings with cosine similarity comparison.
//!
//! Requires a GGUF model file in the `models/` directory (or HF cache).
//!
//! Requires a multi-thread tokio runtime (`block_in_place`).

use ene_ai::EmbeddingProvider;
use ene_ai::cosine_similarity;
use ene_ai::{GgufEmbeddingProvider, resolve_gguf_path};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model_name = "jina-embeddings-v5-text-small";
    let quantization = "F16";
    let model_dir = std::path::PathBuf::from("./models");

    let gguf_path = resolve_gguf_path(model_name, quantization, model_dir)?;

    println!("Loading model: {model_name}");
    println!("GGUF path: {}", gguf_path.display());

    let provider =
        GgufEmbeddingProvider::load(model_name, gguf_path.to_str().unwrap_or(""), quantization)?;

    println!("Model dimensions: {}", provider.dimensions());
    println!("Model name: {}", provider.model_name());

    let text1 = "The cat sat on the mat.";
    let text2 = "A feline rested on a rug.";
    let text3 = "The stock market crashed today.";

    let emb1 = ene_ai::embed_query(&provider, text1).await?;
    let emb2 = ene_ai::embed_query(&provider, text2).await?;
    let emb3 = ene_ai::embed_query(&provider, text3).await?;

    let similarity = cosine_similarity(&emb1, &emb2);
    let similarity_unrelated = cosine_similarity(&emb1, &emb3);

    println!("\nText 1: \"{text1}\"");
    println!("Text 2: \"{text2}\"");
    println!("Cosine similarity: {similarity:.4}");
    println!("\nText 3: \"{text3}\"");
    println!("Cosine similarity (cat vs stocks): {similarity_unrelated:.4}");

    if similarity <= similarity_unrelated {
        return Err("Related texts should be more similar than unrelated ones".into());
    }

    println!("\nEmbeddings work correctly!");

    Ok(())
}
