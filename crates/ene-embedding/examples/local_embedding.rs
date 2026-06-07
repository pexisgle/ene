//! Local embedding example using ene-embedding.
//!
//! Demonstrates loading a GGUF-quantized model from disk and computing
//! embeddings with cosine similarity comparison.
//!
//! Requires a GGUF model file in the `models/` directory.
//! Download from `HuggingFace`:
//!   huggingface-cli download jinaai/jina-embeddings-v5-text-small

use ene_embedding::{GgufEmbeddingProvider, resolve_gguf_paths};
use ene_provider::EmbeddingProvider;

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // Compute embeddings for two sentences
    let text1 = "The cat sat on the mat.";
    let text2 = "A feline rested on a rug.";

    let emb1 = tokio::runtime::Runtime::new()?.block_on(provider.embed_query(text1))?;

    let emb2 = tokio::runtime::Runtime::new()?.block_on(provider.embed_query(text2))?;

    let similarity = cosine_similarity(&emb1, &emb2);

    println!("\nText 1: \"{text1}\"");
    println!("Text 2: \"{text2}\"");
    println!("Cosine similarity: {similarity:.4}");

    // Compare with unrelated text
    let text3 = "The stock market crashed today.";
    let emb3 = tokio::runtime::Runtime::new()?.block_on(provider.embed_query(text3))?;

    let similarity_unrelated = cosine_similarity(&emb1, &emb3);
    println!("\nText 3: \"{text3}\"");
    println!("Cosine similarity (cat vs stocks): {similarity_unrelated:.4}");

    assert!(
        similarity > similarity_unrelated,
        "Related texts should be more similar than unrelated ones"
    );

    println!("\nEmbeddings work correctly!");

    Ok(())
}
