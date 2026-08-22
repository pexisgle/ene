//! Bulk listen WebSocket pump.

use std::sync::Arc;

use ene_api::{ApiClient, LISTEN_SAMPLE_RATE};
use tokio::sync::mpsc::Receiver;

/// Forward coalesced 16 kHz frames until `rx` is closed (mic released).
pub async fn run_listen_stream(
    client: Arc<ApiClient>,
    session_id: String,
    mut rx: Receiver<Vec<f32>>,
) -> Result<(), String> {
    let mut stream = client
        .listen_stream(&session_id, LISTEN_SAMPLE_RATE)
        .await
        .map_err(|err| err.to_string())?;
    loop {
        tokio::select! {
            pcm = rx.recv() => {
                let Some(pcm) = pcm else {
                    break;
                };
                if pcm.is_empty() {
                    continue;
                }
                stream.send_pcm(&pcm).await.map_err(|err| err.to_string())?;
            }
            incoming = stream.recv() => {
                match incoming {
                    Ok(None) => break,
                    Ok(Some(())) => {}
                    Err(err) => return Err(err.to_string()),
                }
            }
        }
    }
    Ok(())
}
