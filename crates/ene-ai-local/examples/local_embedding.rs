//! Local embedding example using ene-ai-local (llama-cpp-4).
//!
//! Set `ENE_EMBEDDING_MODEL_URL` to an HTTPS `.gguf` URL, or pass one as the first CLI arg.
//!
//! Requires a multi-thread tokio runtime (`block_in_place`).

#![expect(
    clippy::print_stdout,
    reason = "example binary prints embedding similarity results to the terminal by design"
)]

use ene_ai::cosine_similarity;
use ene_ai::{ProactiveAcceleration, ResolvedLocalModel};
use ene_ai_local::{create_local_provider, ensure_gguf_available};

const DEFAULT_URL: &str = "https://huggingface.co/jinaai/jina-embeddings-v5-text-small-retrieval/resolve/main/v5-small-retrieval-F16.gguf";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let embedding_model_url = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("ENE_EMBEDDING_MODEL_URL").ok())
        .unwrap_or_else(|| DEFAULT_URL.to_string());

    let local = ResolvedLocalModel {
        name: "jina-v5-small".to_string(),
        url: embedding_model_url,
        model_path: String::new(),
        mmproj_url: String::new(),
        mmproj_path: String::new(),
        quantization: "F16".to_string(),
        acceleration: ProactiveAcceleration::Cpu,
        gpu_layers: "auto".to_string(),
        context_size: 2048,
    };

    ensure_gguf_available(&local).await?;
    let provider = create_local_provider(&local)?;
    println!("Loading model: {}", local.name);
    println!("Model dimensions: {}", provider.dimensions());
    println!("Model name: {}", provider.model_name());

    let text1 = "The cat sat on the mat.";
    let text2 = "A feline rested on a rug.";
    let text3 = "The stock market crashed today.";

    let emb1 = ene_ai::embed_query(provider.as_ref(), text1).await?;
    let emb2 = ene_ai::embed_query(provider.as_ref(), text2).await?;
    let emb3 = ene_ai::embed_query(provider.as_ref(), text3).await?;

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
