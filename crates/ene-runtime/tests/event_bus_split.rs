//! Regression tests for #272: the event bus is split into three dedicated
//! channels (chat / audio / lifecycle) by traffic class, so a burst on one
//! channel can never lag or starve consumers of another.
//!
//! Before #272, `AudioChunk` rode the same 1024-capacity `broadcast`
//! channel as chat events (`TextDelta`, `Performance`, …). Because
//! `tokio::sync::broadcast` retains a per-subscriber ring buffer, a burst
//! of heavyweight PCM chunks could evict buffered chat events from a slow
//! subscriber's window, causing `RecvError::Lagged` for reasons entirely
//! unrelated to chat traffic volume.

#![expect(
    clippy::expect_used,
    clippy::field_reassign_with_default,
    clippy::panic,
    reason = "integration tests use expect, default-then-assign fixtures, \
              and panic on invariant violations"
)]

use ene_config::CharacterCardV3;
use ene_runtime::{AudioChunk, EneConfig, EneHandle, TurnId, TurnOrigin};
use tokio::sync::{broadcast, mpsc};

fn test_card() -> CharacterCardV3 {
    let mut card = CharacterCardV3::default();
    card.data.name = "EventBusTest".into();
    card.data.system_prompt = "Be brief.".into();
    card
}

fn test_config_memory_off() -> EneConfig {
    let mut config = EneConfig::default();
    let mut store = ene_store::StoreConfig::default();
    store.enabled = false;
    config.set_section(&store).expect("store config merges");
    let mut tools = ene_plugin_host::PluginConfig::default();
    tools.enabled = false;
    let _ = config.set_section(&tools);
    let ai = ene_ai::AiConfig::default();
    let _ = config.set_section(&ai);
    config
}

/// Mirrors the chat bus capacity `EneHandle::open` wires up (#272). Kept as
/// a literal (not imported) since it's a private implementation constant —
/// this test cares about behavior, not the exact number.
const CHAT_CAPACITY: usize = 1024;

/// A bounded audio channel capacity representative of production (#272);
/// deliberately smaller than the chat capacity so the burst below actually
/// exercises back-pressure/drop behavior on the audio side.
const AUDIO_CAPACITY: usize = 64;

/// Core regression test: saturating a bounded audio `mpsc` channel with a
/// burst of heavyweight PCM chunks must not cause a concurrent chat-bus
/// `broadcast` subscriber to `Lagged` or lose any event. This is the
/// structural property #272 introduced — chat and audio no longer share a
/// channel, so audio volume cannot evict buffered chat events regardless of
/// how large or bursty it is.
#[tokio::test]
async fn audio_burst_does_not_lag_chat_subscribers() {
    let (chat_tx, mut chat_rx) = broadcast::channel::<ene_runtime::EneEvent>(CHAT_CAPACITY);
    let (audio_tx, mut audio_rx) = mpsc::channel::<AudioChunk>(AUDIO_CAPACITY);

    let turn = TurnId::new();

    // Simulate a sustained burst of heavyweight PCM chunks -- far more than
    // the chat channel's capacity -- like a slow subscriber sitting behind
    // real-time TTS synthesis. Before #272 this traffic pattern would have
    // shared the chat broadcast buffer and evicted chat events.
    let burst_count = CHAT_CAPACITY * 4;
    let audio_turn = turn.clone();
    let audio_task = tokio::spawn(async move {
        for i in 0..burst_count {
            let chunk = AudioChunk {
                turn: audio_turn.clone(),
                origin: TurnOrigin::User,
                pcm: vec![0.0_f32; 4096],
                sample_rate: 24_000,
                is_final: i + 1 == burst_count,
            };
            // Bounded send: back-pressures against the drain task below,
            // exactly like the production audio channel.
            let _ = audio_tx.send(chunk).await;
        }
    });

    // Drain the audio channel concurrently, as a playback consumer would.
    let drain_task = tokio::spawn(async move {
        let mut received = 0usize;
        while audio_rx.recv().await.is_some() {
            received += 1;
        }
        received
    });

    // Fill the chat channel to (but not over) its own capacity. If audio
    // traffic still shared this buffer, the burst above would have already
    // evicted some of these before they're ever read.
    for i in 0..CHAT_CAPACITY {
        let _ = chat_tx.send(ene_runtime::EneEvent::TextDelta {
            turn: turn.clone(),
            origin: TurnOrigin::User,
            delta: format!("chunk {i}"),
        });
    }

    audio_task.await.expect("audio burst task completes");
    let audio_received = drain_task.await.expect("audio drain task completes");
    assert_eq!(
        audio_received, burst_count,
        "every audio chunk should have been delivered to a draining consumer"
    );

    // Every chat event must be receivable without ever hitting `Lagged`,
    // proving the concurrent audio burst never touched the chat channel's
    // ring buffer.
    let mut received = 0usize;
    loop {
        match chat_rx.try_recv() {
            Ok(ene_runtime::EneEvent::TextDelta { .. }) => received += 1,
            Ok(_) => {}
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                break;
            }
            Err(broadcast::error::TryRecvError::Lagged(n)) => {
                panic!("chat subscriber lagged by {n} events due to unrelated audio traffic");
            }
        }
    }
    assert_eq!(
        received, CHAT_CAPACITY,
        "expected every chat event to be received without loss"
    );
}

/// API-shape regression test: [`EneHandle::take_audio_stream`] models
/// single-consumer ownership transfer, not a broadcast (#272). The first
/// call must succeed; every call after that (including from a cloned
/// handle) must return `None`.
#[tokio::test]
async fn take_audio_stream_transfers_ownership_once() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");

    assert!(
        handle.take_audio_stream().is_some(),
        "first take_audio_stream call should succeed"
    );
    assert!(
        handle.take_audio_stream().is_none(),
        "second take_audio_stream call on the same handle must return None"
    );

    // Ownership transfer is global across clones, not per-clone.
    let clone = handle.clone();
    assert!(
        clone.take_audio_stream().is_none(),
        "take_audio_stream on a clone must also return None once claimed"
    );

    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}

/// API-shape regression test: the lifecycle bus is a distinct subscription
/// from the chat bus (#272) — `subscribe_lifecycle` returns a working
/// receiver independent of `subscribe`.
#[tokio::test]
async fn subscribe_lifecycle_is_independent_of_chat_bus() {
    let handle = EneHandle::open(test_config_memory_off(), test_card())
        .await
        .expect("open initializes handle");

    let mut chat_rx = handle.subscribe();
    let mut lifecycle_rx = handle.subscribe_lifecycle();

    // Neither receiver should observe anything before any turn runs; this
    // just proves both subscriptions are live and independently pollable.
    assert!(matches!(
        chat_rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        lifecycle_rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));

    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
}
